# xmip-core-route-content

Content route technology: `content:<language>:<expression>` follows a dot or JSONPath path into the first section's content through xmip-core-path and reads the scalar it finds. A technology of [xmip-core-route](https://github.com/IlleNilsson/xmip-core-route).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
