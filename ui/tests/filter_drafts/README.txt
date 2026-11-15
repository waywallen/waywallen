Filter draft validation regression test

Requires Qt Core, Qml, Protobuf and ProtobufQuick development packages.
From the repository root:
  cmake -S ui/tests/filter_drafts -B build-filter-drafts -DCMAKE_BUILD_TYPE=Release
  cmake --build build-filter-drafts
  ctest --test-dir build-filter-drafts --output-on-failure

This tests the production rule/state validation helpers with generated protobuf
messages. It covers all eight types, supported condition boundaries, incomplete
and mismatched payloads, library IDs, legacy string-backed tags, group references
and preservation of serialized bytes during validation. It does not instantiate
the full Qt/QML model; Add/apply signals and the editor require integration testing.
