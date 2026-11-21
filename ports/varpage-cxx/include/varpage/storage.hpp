#pragma once

#include <cstddef>
#include <cstdint>
#include <iosfwd>
#include <map>
#include <optional>
#include <span>
#include <unordered_map>
#include <vector>

namespace varpage {

using RecordId = std::uint64_t;

// A store borrows a seekable stream. Keep the stream alive and give the store
// exclusive access to its bytes. An interrupted or failed write may corrupt it.
class Storage {
public:
    // Write an empty store header at start. Existing stream bytes beyond the
    // header are left in place.
    static Storage create(std::iostream& stream, std::uint64_t start = 0);
    static Storage open(std::iostream& stream, std::uint64_t start = 0);

    Storage(const Storage&) = delete;
    Storage& operator=(const Storage&) = delete;
    Storage(Storage&&) = default;

    std::optional<std::vector<std::byte>> read(RecordId id);
    void write(RecordId id, std::span<const std::byte> payload);
    std::vector<RecordId> record_ids() const;

    // Clear bytes released by later writes or by vacuum.
    void set_zero_free_space(bool enabled) noexcept { zero_free_space_ = enabled; }
    void flush();

    // Absolute stream positions. Vacuum packs the records without truncating.
    std::uint64_t end_position() const noexcept;
    std::uint64_t vacuum();

private:
    struct Record {
        RecordId id;
        std::uint64_t payload_position;
        std::uint64_t trailer;
        std::uint64_t next_offset;
        std::uint64_t end;
    };
    using Records = std::map<std::uint64_t, Record>;
    using Iterator = Records::const_iterator;

    enum class Mode { create, open };
    Storage(std::iostream& stream, std::uint64_t start, Mode mode);
    void load();

    std::uint64_t gap_start(Iterator previous) const;
    void refresh_gap(std::uint64_t end);
    Iterator predecessor(std::uint64_t position, Iterator excluded) const;
    Iterator successor(std::uint64_t position, Iterator excluded) const;
    std::uint64_t choose_position(Iterator old, std::uint64_t required) const;

    std::iostream& stream_;
    std::uint64_t start_;
    std::uint64_t header_end_;
    bool zero_free_space_ = false;
    Records records_;
    std::unordered_map<RecordId, std::uint64_t> positions_;
    // Nonempty gaps, keyed by the start of the record to their right.
    std::map<std::uint64_t, std::uint64_t> gaps_;
};

} // namespace varpage
