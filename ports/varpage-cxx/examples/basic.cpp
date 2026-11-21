#include <varpage/storage.hpp>

#include <iostream>
#include <sstream>
#include <string_view>

using namespace std;
using namespace varpage;

int main() {
    stringstream stream;
    {
        auto store = Storage::create(stream);
        const string_view message = "hello";
        store.write(7, as_bytes(span{message}));
    }

    auto store = Storage::open(stream);
    if (const auto message = store.read(7)) {
        cout.write(reinterpret_cast<const char*>(message->data()), static_cast<streamsize>(message->size()));
        cout << endl;
    }
}
