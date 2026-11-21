# Bundled fonts

The interface and message fonts the web client offers, as static Regular,
SemiBold and Bold files so GPUI can match them by weight on every platform.
All are under the SIL Open Font License 1.1; each family's licence is in
`licenses/`.

They are made from the variable fonts in [google/fonts](https://github.com/google/fonts)
(`ofl/<family>/<Family>[wght].ttf`) with `instance.py`:

```sh
uv run --with fonttools python -I instance.py <downloaded ofl dirs> <out dir>
```

`crates/app/src/theme/fonts.rs` lists every file; add a family there too.
