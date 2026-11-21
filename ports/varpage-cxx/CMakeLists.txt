cmake_minimum_required(VERSION 3.21)
project(varpage-cxx LANGUAGES CXX VERSION 0.1.0)

add_library(varpage_cxx STATIC src/storage.cpp)
add_library(varpage::varpage ALIAS varpage_cxx)
set_target_properties(varpage_cxx PROPERTIES POSITION_INDEPENDENT_CODE ON)
target_compile_features(varpage_cxx PUBLIC cxx_std_20)
target_include_directories(varpage_cxx PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}/include")

if(PROJECT_IS_TOP_LEVEL)
    add_executable(varpage_cxx_basic examples/basic.cpp)
    target_link_libraries(varpage_cxx_basic PRIVATE varpage::varpage)
    add_executable(varpage_cxx_compact examples/compact.cpp)
    target_link_libraries(varpage_cxx_compact PRIVATE varpage::varpage)

    # Generate compile-commands.json for clangd
    set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
endif()

option(VARPAGE_CXX_BUILD_TESTS "Build the varpage C++ tests" ${PROJECT_IS_TOP_LEVEL})
if(VARPAGE_CXX_BUILD_TESTS)
    enable_testing()
    add_executable(varpage_cxx_tests tests/storage.cpp)
    target_link_libraries(varpage_cxx_tests PRIVATE varpage::varpage)
    add_test(NAME varpage_cxx.storage COMMAND varpage_cxx_tests)
endif()
