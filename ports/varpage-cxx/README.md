# varpage for C++

A tier 2 C++20 port of varpage. The Rust crate is the reference implementation.
Both use the same [on-disk format](../../docs/format.md).

## Use with CMake

With CMake 3.21 or later, add this to your project's `CMakeLists.txt`.

```cmake
include(FetchContent)
FetchContent_Declare(
    varpage
    GIT_REPOSITORY https://github.com/LucaCiucci/varpage.git
    GIT_TAG main
    SOURCE_SUBDIR ports/varpage-cxx
)
FetchContent_MakeAvailable(varpage)
target_link_libraries(my_app PRIVATE varpage::varpage)
```

## Store records

```cpp
#include <varpage/storage.hpp>
#include <sstream>
#include <string_view>

std::stringstream stream;
auto store = varpage::Storage::create(stream);
const std::string_view text = "hello";
store.write(7, std::as_bytes(std::span{text}));
auto data = store.read(7);  // optional<vector<byte>>
```

The store borrows a seekable `std::iostream`. Use `std::stringstream` in memory
or `std::fstream` in binary read/write mode for files. Keep the stream alive
and give the store exclusive access to its bytes. Call `Storage::open` to
reopen an existing store. Both factories take an optional absolute start
position, defaulting to zero.

Writes update records and links locally. Placement scans gaps in stream order.
`vacuum()` packs records in place and returns the absolute position where you
can truncate the underlying file after flushing and closing it. Operations
report errors by throwing exceptions. A failed or interrupted mutation can
leave a store inconsistent.

## Build, test, and run the examples

From this directory:

```sh
cmake -S . -B build
cmake --build build
ctest --test-dir build --output-on-failure
./build/varpage_cxx_basic
./build/varpage_cxx_compact output.bin
```

The compact example creates or overwrites its output file. CMake adds both
examples only in a standalone build. The dependency-free tests are enabled
by default in that build and can be disabled with
`-DVARPAGE_CXX_BUILD_TESTS=OFF`.
