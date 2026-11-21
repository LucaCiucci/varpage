#include <varpage/storage.hpp>

#include <algorithm>
#include <iostream>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>

namespace {

using varpage::Storage;

auto bytes(std::string_view text) {
    return std::as_bytes(std::span{text});
}

void check(bool condition, const char* message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

void check_record(Storage& store, varpage::RecordId id, std::string_view expected) {
    const auto data = store.read(id);
    check(data && std::ranges::equal(*data, bytes(expected)), "wrong record payload");
}

void format_round_trip() {
    // The reference format example followed by a record with the largest ID.
    const std::string wire = "\x01\x07\x02hi\x01\x09\x01!\x01"
        + std::string(9, '\xff') + std::string("\x01\0\0", 3);
    const auto max_id = std::numeric_limits<varpage::RecordId>::max();
    std::stringstream input(wire);
    auto reader = Storage::open(input);
    check_record(reader, 7, "hi");
    check_record(reader, 9, "!");
    check_record(reader, max_id, "");
    check(!reader.read(42), "missing record was found");

    std::stringstream output;
    auto writer = Storage::create(output);
    writer.write(7, bytes("hi"));
    writer.write(9, bytes("!"));
    writer.write(max_id, {});
    check(output.str() == wire && writer.end_position() == wire.size(), "wrong wire format");
}

void relocate_reuse_and_vacuum() {
    std::stringstream stream("HDR");
    const std::string large(20000, 'y');
    {
        auto store = Storage::create(stream, 3);
        store.set_zero_free_space(true);
        store.write(1, bytes(std::string(200, 'x')));
        store.write(2, bytes("keep"));
        store.write(1, bytes(large));
    }
    const auto relocated = stream.str();
    check(relocated.substr(0, 5) == "HDR\xcd\x01", "wrong first offset after relocation");
    check(relocated.substr(5, 203) == std::string(203, '\0'), "released bytes were not cleared");

    {
        auto reopened = Storage::open(stream, 3);
        reopened.set_zero_free_space(true);
        check_record(reopened, 1, large);
        check_record(reopened, 2, "keep");
        reopened.write(3, bytes("gap"));
        check(reopened.record_ids() == std::vector<varpage::RecordId>{3, 2, 1}, "leading gap was not reused");
        const auto old_end = reopened.end_position();
        const auto end = reopened.vacuum();
        reopened.flush();
        check(end < old_end && end == reopened.end_position(), "wrong packed end");
        const auto packed = stream.str();
        check(packed.starts_with("HDR") && packed.substr(end) == std::string(old_end - end, '\0'),
              "vacuum changed the prefix or left a nonzero tail");
    }

    auto compacted = Storage::open(stream, 3);
    check_record(compacted, 3, "gap");
    check_record(compacted, 2, "keep");
    check_record(compacted, 1, large);
}

void malformed_links() {
    for (const auto& wire : {std::string("\x81\0", 2), std::string("\x01\x07\x02", 3),
                             std::string("\x01\x07\0\x01\x07\0\0", 7)}) {
        std::stringstream stream(wire);
        try {
            Storage::open(stream);
        } catch (const std::runtime_error&) {
            continue;
        }
        throw std::runtime_error("malformed store opened successfully");
    }
}

} // namespace

int main() {
    try {
        format_round_trip();
        relocate_reuse_and_vacuum();
        malformed_links();
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
