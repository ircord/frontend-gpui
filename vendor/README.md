# Vendored crates

## gpui-pre-windows

GPUI's Windows backend (`gpui-pre-windows` 0.3.12, the version `gpui-kit`
0.10.0 pins), with `gpui-pre-windows.patch` applied. The workspace uses it
through `[patch.crates-io]`.

Why: the published crate compiles its HLSL shaders in `build.rs` with
Microsoft's `fxc.exe`, behind `#[cfg(target_os = "windows")]`. In a build
script that tests the machine doing the build, so a Linux cross-build skips
the step and the release code then includes a `shaders_bytes.rs` that was
never written. The patch compiles the embedded HLSL in memory at start-up
with `D3DCompile` from `d3dcompiler_47.dll`, which ships with Windows. It
changes one file, `src/directx_renderer.rs`.

If the patched `.exe` shows a blank window on Windows, the fallback is to
precompile the shaders once with `fxc.exe`, commit the resulting
`shaders_bytes.rs`, and have the vendored `build.rs` copy it into place.

### Bumping gpui-kit

1. Note the `gpui-pre-windows` version the new `gpui-kit` pins.
2. Replace `gpui-pre-windows/` with that version's published source.
3. Apply the patch: `(cd vendor/gpui-pre-windows && patch -p1 < ../gpui-pre-windows.patch)`;
   fix it up by hand if it no longer applies, and refresh the patch file.
4. Run the whole ladder, and run the `.exe` once on Windows before landing.
