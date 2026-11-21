# Building

Requirements:

* Windows x64
* Rust toolchain `stable-x86_64-pc-windows-msvc`
* Visual Studio Build Tools with the MSVC x64 toolchain
* Windows SDK 10

## One-time environment

`rustup target list --installed` should include `x86_64-pc-windows-msvc`
(it is the default on Windows).

`link.exe` and `kernel32.lib` / `user32.lib` must be reachable. A `build.cmd`
that sets `PATH` / `LIB` / `INCLUDE` for a default Build Tools + SDK install is
included in the mod repository; the same environment can be set manually:

```
set "MSVC=C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Tools\MSVC\<version>"
set "SDK=C:\Program Files (x86)\Windows Kits\10"
set "PATH=%MSVC%\bin\Hostx64\x64;%SDK%\bin\10.0.26100.0\x64;%PATH%"
set "LIB=%MSVC%\lib\x64;%SDK%\Lib\10.0.26100.0\ucrt\x64;%SDK%\Lib\10.0.26100.0\um\x64"
set "INCLUDE=%MSVC%\include;%SDK%\Include\10.0.26100.0\ucrt;%SDK%\Include\10.0.26100.0\um;%SDK%\Include\10.0.26100.0\shared"
```

## Build

```
cargo test --lib
cargo build --release
```

Output: `target\release\speedforce.dll`.

The release plug-in shipped on the mod page is this file renamed to
`SpeedForce.dll` (the name ModEngine3 loads). The exported symbol names
(`SpeedForce_M4J_Finalize`, `SpeedForce_M4G_Version`, …) are kept stable on
purpose so existing ModEngine3 profiles keep working; do not rename them.

## Reproducibility

`Cargo.lock` is committed, the crate has no dependencies, and the release
profile uses `lto = true` + `codegen-units = 1`. Rebuilds on the same toolchain
produce functionally identical binaries; byte-for-byte equality additionally
needs the same rustc / MSVC / SDK versions.
