# FlashTrans source package

Version: 2.1.0

This directory contains the complete project source, configuration, lockfiles,
bundled fonts, font licenses, and generated application icons.

The selected icon source is `src-tauri/icons/logo2-original.png`. Because the
original image is not square, `src-tauri/icons/logo2-square.png` is the centered
square crop used to generate the platform icon set.

Generated dependency and build directories are intentionally excluded:
`node_modules`, `dist`, and `src-tauri/target`.

Build on Windows:

```powershell
npm install
npm run build
npm run tauri build -- --bundles nsis
```

The release executable and installer are written to `src-tauri/target/release`.
