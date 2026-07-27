# @juya-ai/tokenx

Tokenx reports local AI coding assistant token usage and estimated cost through a CLI and interactive TUI.

## Install

```sh
npm install --global @juya-ai/tokenx
tokenx
```

The npm launcher installs a matching native binary as an optional platform package. Prebuilt binaries are available for:

- macOS on Apple silicon (`arm64`)
- Linux on x64 with glibc
- Windows on x64

Other platforms are not currently published through npm. Tokenx does not execute an unrelated `tokenx` found on `PATH` when its native package is unavailable.

Usage, configuration, client support, and source builds are documented in the [Tokenx repository](https://github.com/makoMakoGo/tokenx).

## License

MIT. Tokenx originated from [Tokscale](https://github.com/junhoyeo/tokscale) by Junho Yeo; see `NOTICE` for attribution.
