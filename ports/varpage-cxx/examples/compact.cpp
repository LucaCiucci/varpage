#include <varpage/storage.hpp>

#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string_view>

using namespace std;
using namespace varpage;

int main(int argc, char** argv) {
    if (argc != 2) {
        cerr << "Usage: varpage_cxx_compact <output-file>\n";
        return 1;
    }

    try {
        fstream file(argv[1], ios::in | ios::out | ios::binary | ios::trunc);
        const auto end = [&] {
            auto store = Storage::create(file);
            const string_view original = "a longer first record";
            const string_view second = "second";
            const string_view replacement = "short";
            store.write(1, as_bytes(span{original}));
            store.write(2, as_bytes(span{second}));
            store.write(1, as_bytes(span{replacement}));

            const auto before = store.end_position();
            const auto after = store.vacuum();
            store.flush();
            cout << "Compacted from " << before << " to " << after << " bytes\n";
            return after;
        }();
        file.close();
        if (!file) {
            throw ios_base::failure("could not close output file");
        }
        filesystem::resize_file(argv[1], end);
    } catch (const exception& error) {
        cerr << error.what() << '\n';
        return 1;
    }
}
