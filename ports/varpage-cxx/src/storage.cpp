#include <varpage/storage.hpp>

#include <algorithm>
#include <array>
#include <ios>
#include <iostream>
#include <iterator>
#include <limits>
#include <stdexcept>
#include <utility>

namespace varpage {
namespace {

using Position = std::uint64_t;

Position add(Position position, Position length) {
    if (length > std::numeric_limits<Position>::max() - position) {
        throw std::overflow_error("varpage position overflow");
    }
    return position + length;
}

std::streamoff stream_offset(Position position) {
    if (!std::in_range<std::streamoff>(position)) {
        throw std::overflow_error("varpage position exceeds stream range");
    }
    return static_cast<std::streamoff>(position);
}

void check_stream(const std::iostream& stream) {
    if (!stream) {
        throw std::ios_base::failure("varpage stream I/O failed");
    }
}

void seek_read(std::iostream& stream, Position position) {
    stream.seekg(stream_offset(position), std::ios::beg);
    check_stream(stream);
}

void seek_write(std::iostream& stream, Position position) {
    stream.seekp(stream_offset(position), std::ios::beg);
    check_stream(stream);
}

std::streamsize stream_count(std::size_t size) {
    if (!std::in_range<std::streamsize>(size)) {
        throw std::length_error("varpage payload exceeds stream I/O range");
    }
    return static_cast<std::streamsize>(size);
}

void read_bytes(std::iostream& stream, std::span<std::byte> bytes) {
    if (!bytes.empty()) {
        stream.read(reinterpret_cast<char*>(bytes.data()), stream_count(bytes.size()));
        check_stream(stream);
    }
}

void write_bytes(std::iostream& stream, std::span<const std::byte> bytes) {
    if (!bytes.empty()) {
        stream.write(reinterpret_cast<const char*>(bytes.data()), stream_count(bytes.size()));
        check_stream(stream);
    }
}

std::size_t encode(Position value, std::byte* bytes) {
    std::size_t length = 0;
    while (value >= 128) {
        bytes[length++] = static_cast<std::byte>((value & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes[length++] = static_cast<std::byte>(value);
    return length;
}

Position encoded_size(Position value) {
    std::array<std::byte, 10> bytes;
    return encode(value, bytes.data());
}

// Track the actual field width so that overlong encodings accepted by Rust
// still give the right payload positions and overlap checks.
Position read_integer(std::iostream& stream, Position& position) {
    Position value = 0;
    for (unsigned shift = 0; shift <= 126; shift += 7) {
        const int byte = stream.get();
        check_stream(stream);
        position = add(position, 1);
        const auto bits = static_cast<Position>(byte & 0x7f);
        if ((shift == 63 && bits > 1) || (shift > 63 && bits != 0)) {
            break;
        }
        if (shift < 64) {
            value |= bits << shift;
        }
        if ((byte & 0x80) == 0) {
            return value;
        }
    }
    throw std::runtime_error("invalid varpage integer");
}

void write_integer(std::iostream& stream, Position value) {
    std::array<std::byte, 10> bytes;
    const auto length = encode(value, bytes.data());
    write_bytes(stream, {bytes.data(), length});
}

struct EncodedHeader {
    std::array<std::byte, 20> bytes;
    std::size_t size;

    EncodedHeader(RecordId id, Position payload_size) {
        const auto id_size = encode(id, bytes.data());
        size = id_size + encode(payload_size, bytes.data() + id_size);
    }

    void write(std::iostream& stream) const {
        write_bytes(stream, {bytes.data(), size});
    }
};

Position link_target(Position start, Position end, Position offset) {
    const auto target = add(start, offset);
    if (offset != 0 && target < end) {
        throw std::runtime_error("overlapping varpage link");
    }
    return target;
}

struct Range {
    Position start = 0;
    Position end = 0;
};

void zero_range(std::iostream& stream, Position start, Position end) {
    if (start >= end) {
        return;
    }
    const std::array<std::byte, 8192> zeroes{};
    seek_write(stream, start);
    while (start < end) {
        const auto count = static_cast<std::size_t>(std::min<Position>(end - start, zeroes.size()));
        write_bytes(stream, std::span{zeroes}.first(count));
        start += count;
    }
}

// Old record and link bytes may now belong to the new record or a longer
// link. Clear only the portions that are outside all newly written ranges.
void clear_released(std::iostream& stream, const std::array<Range, 4>& old,
                    std::array<Range, 4> live) {
    std::ranges::sort(live, {}, &Range::start);
    for (const auto& range : old) {
        auto start = range.start;
        for (const auto& used : live) {
            zero_range(stream, start, std::min(range.end, used.start));
            start = std::max(start, used.end);
            if (start >= range.end) {
                break;
            }
        }
        zero_range(stream, start, range.end);
    }
}

// Copy forward because the packed payload starts at or before its old
// position. Each chunk is read before any overlapping bytes are overwritten.
void pack_payload(std::iostream& stream, Position source, Position target, Position size) {
    if (source == target) {
        return;
    }
    std::array<std::byte, 8192> buffer;
    while (size > 0) {
        const auto count = static_cast<std::size_t>(std::min<Position>(size, buffer.size()));
        const auto chunk = std::span{buffer}.first(count);
        seek_read(stream, source);
        read_bytes(stream, chunk);
        seek_write(stream, target);
        write_bytes(stream, chunk);
        source += count;
        target += count;
        size -= count;
    }
}

} // namespace

Storage Storage::create(std::iostream& stream, Position start) {
    return Storage(stream, start, Mode::create);
}

Storage Storage::open(std::iostream& stream, Position start) {
    return Storage(stream, start, Mode::open);
}

Storage::Storage(std::iostream& stream, Position start, Mode mode)
    : stream_(stream), start_(start), header_end_(add(start, 1)) {
    if (mode == Mode::open) {
        load();
    } else {
        stream_offset(header_end_);
        seek_write(stream_, start_);
        write_integer(stream_, 0);
    }
}

void Storage::load() {
    seek_read(stream_, start_);
    header_end_ = start_;
    auto offset = read_integer(stream_, header_end_);
    auto position = link_target(start_, header_end_, offset);
    auto first_free = start_ + 1;
    while (offset != 0) {
        seek_read(stream_, position);
        auto payload = position;
        const auto id = read_integer(stream_, payload);
        const auto size = read_integer(stream_, payload);
        const auto trailer = add(payload, size);
        seek_read(stream_, trailer);
        auto end = trailer;
        offset = read_integer(stream_, end);
        const auto next = link_target(trailer, end, offset);
        if (!positions_.emplace(id, position).second) {
            throw std::runtime_error("duplicate varpage record ID");
        }
        records_.emplace_hint(records_.end(), position, Record{id, payload, trailer, offset, end});
        if (first_free < position) {
            gaps_.emplace_hint(gaps_.end(), position, first_free);
        }
        first_free = trailer + 1;
        position = next;
    }
}

std::optional<std::vector<std::byte>> Storage::read(RecordId id) {
    const auto found = positions_.find(id);
    if (found == positions_.end()) {
        return std::nullopt;
    }
    const auto& record = records_.at(found->second);
    const auto size = record.trailer - record.payload_position;
    if (!std::in_range<std::size_t>(size)) {
        throw std::length_error("varpage payload exceeds address space");
    }
    std::vector<std::byte> bytes(static_cast<std::size_t>(size));
    seek_read(stream_, record.payload_position);
    read_bytes(stream_, bytes);
    return bytes;
}

std::vector<RecordId> Storage::record_ids() const {
    std::vector<RecordId> ids;
    ids.reserve(records_.size());
    for (const auto& entry : records_) {
        ids.push_back(entry.second.id);
    }
    return ids;
}

Storage::Iterator Storage::predecessor(Position position, Iterator excluded) const {
    auto found = records_.lower_bound(position);
    if (found != records_.begin() && std::prev(found) == excluded) {
        --found;
    }
    return found == records_.begin() ? records_.end() : std::prev(found);
}

Storage::Iterator Storage::successor(Position position, Iterator excluded) const {
    auto found = records_.upper_bound(position);
    if (found != records_.end() && found == excluded) {
        ++found;
    }
    return found;
}

Position Storage::gap_start(Iterator previous) const {
    // An incoming link of distance one occupies one byte. Moving the next
    // record closer may reclaim bytes from a longer existing link.
    return (previous == records_.end() ? start_ : previous->second.trailer) + 1;
}

void Storage::refresh_gap(Position end) {
    const auto start = gap_start(predecessor(end, records_.end()));
    if (start < end) {
        gaps_.insert_or_assign(end, start);
    } else {
        gaps_.erase(end);
    }
}

Position Storage::choose_position(Iterator old, Position required) const {
    const auto last = records_.empty() ? records_.end() : std::prev(records_.end());
    const auto tail = last != records_.end() && last == old
        ? predecessor(last->first, records_.end()) : last;
    auto candidate = gap_start(tail);

    if (old != records_.end()) {
        const auto next = std::next(old);
        const auto limit = next == records_.end()
            ? std::numeric_limits<Position>::max() : next->first;
        if (required <= limit - old->first) {
            return old->first;
        }
        // Removing the old record joins its extent to the gaps on either side.
        const auto merged = gap_start(predecessor(old->first, records_.end()));
        if (required <= limit - merged) {
            candidate = merged;
        }
    }

    for (const auto& [end, start] : gaps_) {
        if (start >= candidate) {
            break;
        }
        if (required <= end - start) {
            return start;
        }
    }
    return candidate;
}

void Storage::write(RecordId id, std::span<const std::byte> bytes) {
    const auto indexed = positions_.find(id);
    const auto old = indexed == positions_.end() ? records_.end() : records_.find(indexed->second);
    const auto old_next = old == records_.end() ? records_.end() : std::next(old);
    const EncodedHeader header(id, bytes.size());
    stream_count(bytes.size());

    const auto required = add(add(header.size, bytes.size()), 1);
    const auto position = choose_position(old, required);
    const auto payload = add(position, header.size);
    const auto trailer = add(payload, bytes.size());
    const auto next = successor(position, old);
    const auto offset = next == records_.end() ? 0 : next->first - trailer;
    const auto end = add(trailer, encoded_size(offset));
    stream_offset(end);

    // At most two existing records change their outgoing link, one at the
    // old position and one at the new position.
    struct LinkUpdate {
        Iterator record;
        Position offset;
        Position end;
    };
    std::array<LinkUpdate, 2> links;
    std::size_t link_count = 0;
    const auto new_previous = predecessor(position, old);
    auto old_previous = old == records_.end() ? records_.end() : predecessor(old->first, old);
    if (old_previous == new_previous) {
        old_previous = records_.end();
    }
    for (const auto previous : {new_previous, old_previous}) {
        if (previous == records_.end()) {
            continue;
        }
        const auto following = successor(previous->first, old);
        auto target = following == records_.end() ? previous->second.trailer : following->first;
        if (position > previous->first && (following == records_.end() || position < target)) {
            target = position;
        }
        const auto link = target - previous->second.trailer;
        if (link != previous->second.next_offset) {
            links[link_count++] = {previous, link, add(previous->second.trailer, encoded_size(link))};
        }
    }
    auto first_other = records_.cbegin();
    if (first_other != records_.end() && first_other == old) {
        ++first_other;
    }
    const auto first = first_other == records_.end() ? position : std::min(position, first_other->first);
    const bool update_header = records_.empty() || first != records_.begin()->first;

    // One record, up to two predecessor links, and the store header.
    std::array<Range, 4> released{};
    std::array<Range, 4> live{};
    if (old != records_.end()) {
        released[0] = {old->first, old->second.end};
    }
    live[0] = {position, end};
    seek_write(stream_, position);
    header.write(stream_);
    write_bytes(stream_, bytes);
    write_integer(stream_, offset);
    for (std::size_t i = 0; i < link_count; ++i) {
        const auto& link = links[i];
        const auto& previous = link.record->second;
        released[i + 1] = {previous.trailer, previous.end};
        live[i + 1] = {previous.trailer, link.end};
        seek_write(stream_, previous.trailer);
        write_integer(stream_, link.offset);
    }
    if (update_header) {
        const auto first_offset = first - start_;
        const auto new_header_end = start_ + encoded_size(first_offset);
        released[3] = {start_, header_end_};
        live[3] = {start_, new_header_end};
        seek_write(stream_, start_);
        write_integer(stream_, first_offset);
        header_end_ = new_header_end;
    }
    if (zero_free_space_) {
        clear_released(stream_, released, live);
    }

    if (old != records_.end() && old->first != position) {
        gaps_.erase(old->first);
        records_.erase(old);
    }
    for (std::size_t i = 0; i < link_count; ++i) {
        const auto& link = links[i];
        auto& previous = records_.at(link.record->first);
        previous.next_offset = link.offset;
        previous.end = link.end;
    }
    positions_.insert_or_assign(id, position);
    records_.insert_or_assign(position, Record{id, payload, trailer, offset, end});
    refresh_gap(position);
    if (next != records_.end()) {
        refresh_gap(next->first);
    }
    if (old_next != records_.end() && old_next != next) {
        refresh_gap(old_next->first);
    }
}

void Storage::flush() {
    stream_.flush();
    check_stream(stream_);
}

Position Storage::end_position() const noexcept {
    return records_.empty() ? header_end_ : records_.rbegin()->second.end;
}

Position Storage::vacuum() {
    const auto old_end = end_position();
    seek_write(stream_, start_);
    write_integer(stream_, records_.empty() ? 0 : 1);

    Records packed;
    auto position = start_ + 1;
    for (auto current = records_.cbegin(); current != records_.cend(); ++current) {
        const auto& old = current->second;
        const auto size = old.trailer - old.payload_position;
        const EncodedHeader header(old.id, size);
        const auto payload = position + header.size;
        const auto trailer = payload + size;
        const Position offset = std::next(current) == records_.end() ? 0 : 1;
        seek_write(stream_, position);
        header.write(stream_);
        pack_payload(stream_, old.payload_position, payload, size);
        seek_write(stream_, trailer);
        write_integer(stream_, offset);
        packed.emplace_hint(packed.end(), position, Record{old.id, payload, trailer, offset, trailer + 1});
        positions_.at(old.id) = position;
        position = trailer + 1;
    }
    if (zero_free_space_) {
        zero_range(stream_, position, old_end);
    }
    records_.swap(packed);
    gaps_.clear();
    header_end_ = start_ + 1;
    return position;
}

} // namespace varpage
