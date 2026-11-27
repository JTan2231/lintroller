# lintroller

`lintroller` checks Swift's `unused_declaration` rule against an existing compiler index. It matches references by compiler symbol ID, so a call in another file or module counts and overloads are checked individually.

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

An unused declaration produces one diagnostic on stdout. Paths are relative to the project root. Lines and UTF-8 byte columns start at one.

```text
Sources/Feature.swift:12:6: error: Unused declaration 'obsolete()' (unused_declaration)
```

The exit status is **0** for a clean scan, **1** for findings, and **2** for invalid input or a failed scan. Errors go to stderr.

## Coverage

Use a fresh index from a successful build of the sources at their indexed paths. Include every target and configuration whose uses should count. The two input directories alone cannot identify missing targets or distinguish obsolete build units left in an incremental store.

Indexed callers outside the project root count as uses. Inside the root, files under `.build`, `.swiftpm`, or `DerivedData` also contribute references only. The check looks for unreferenced declarations, so two functions that call each other both count as used. Public and open API, protocol witnesses, overrides and runtime entry points such as Interface Builder actions are conservatively retained.

## Integration test

```sh
cargo test --test unused_declaration -- --nocapture
```

The test generates 100 Swift project files and an additional caller outside the project root, compiles them into a real index store, and checks the CLI's output against the 21 expected diagnostics. The ten-second limit includes fixture creation, Swift compilation, scanning and cleanup. Cargo builds the Rust executable before the timed test starts.
