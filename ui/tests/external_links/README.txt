External link scheme filter test

Requires Qt Core development packages. From the repository root:
  cmake -S ui/tests/external_links -B build-external-links
  cmake --build build-external-links
  ctest --test-dir build-external-links --output-on-failure
