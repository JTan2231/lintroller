# lintroller

`lintroller` runs 61 SwiftLint rules on the Swift files in a compiler index. Source rules share one syntax tree per file. The two analyzer rules, `unused_declaration` and `unused_import`, also use compiler symbol references.

## Usage

Build the CLI with Cargo and the Swift toolchain that produced the index. Cargo locates `libIndexStore` via `xcrun` on macOS or `swiftc` on `PATH` on Linux. To select a specific library, set `INDEXSTORE_LIB_DIR` when building the CLI.

```sh
cargo build --release
./target/release/lintroller --index-store /path/to/index/store --project-root /path/to/project
```

Both directories are required. Pass the directory given to `swiftc -index-store-path`, the one containing the versioned store (typically `v5/`). From a Swift package directory, these commands include test callers in the scan.

```sh
swift build --build-tests --enable-index-store
/path/to/lintroller --index-store "$(swift build --show-bin-path)/index/store" --project-root .
```

All 61 rules run by default, including SwiftLint opt-in rules. Use `--rules` to select a comma-separated list or repeat the flag. The CLI's `--help` lists the supported identifiers.

```sh
lintroller --index-store /path/to/index/store --project-root . --rules line_length,type_body_length
lintroller --index-store /path/to/index/store --project-root . --rules unused_declaration
```

The rules use built-in settings. Project-specific thresholds and other settings from `.swiftlint.yml` are outside this implementation. Source comments support `swiftlint:disable`, `swiftlint:enable`, `all`, and the `:previous`, `:this`, and `:next` scopes. The superfluous-disable check considers the rules selected for the run.

Each finding produces one diagnostic on stdout. Paths are relative to the project root. Lines and UTF-8 byte columns start at one.

```text
Sources/Feature.swift:12:6: error: Unused declaration 'obsolete()' (unused_declaration)
```

The exit status is **0** for a clean scan, **1** for findings, and **2** for invalid input or a failed scan. Errors go to stderr.

## Coverage

Use a fresh index from a successful build of the sources at their indexed paths. Include every target and configuration whose uses should count. The two input directories alone cannot identify missing targets or distinguish obsolete build units left in an incremental store.

Indexed callers outside the project root count as uses. Inside the root, files under `.build`, `.swiftpm`, or `DerivedData` also contribute references only. The `unused_declaration` check matches compiler symbol IDs across files and targets, including individual overloads. It looks for unreferenced declarations, so two functions that call each other both count as used. Public and open API, protocol witnesses, overrides and runtime entry points such as Interface Builder actions are conservatively retained.

For `unused_import`, the compiler record identifies active imports and references. The check retains scoped imports, explicit reexports, and imports that may provide a symbol whose origin is ambiguous, including some C and Objective-C references.

## Integration test

```sh
cargo test --test unused_declaration -- --nocapture
```

The test generates 100 Swift project files and an additional caller outside the project root, compiles them into a real index store, runs `--rules unused_declaration`, and checks the output against the 21 expected diagnostics. The ten-second limit includes fixture creation, Swift compilation, scanning and cleanup. Cargo builds the Rust executable before the timed test starts.
