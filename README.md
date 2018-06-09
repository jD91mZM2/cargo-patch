# cargo-patch

A WIP cargo subcommand for recursively patching dependencies without `[patch]`.

Usage is simple, `cargo patch --replace crate=url`, for example
`cargo patch --replace mio=https://github.com/redox-os/mio`.
You can use `--replace` multiple times.
