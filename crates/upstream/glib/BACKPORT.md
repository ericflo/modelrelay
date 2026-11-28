This is the crates.io glib 0.18.5 release used by the GTK 3 bindings. Its source archive SHA-256 is `233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`.

The change to `src/variant_iter.rs` backports the upstream fix for RUSTSEC-2024-0429. Drop the local Cargo patch when the desktop dependencies accept glib 0.20 or later. [Upstream fix](https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e439bef2b5eea76c3aa25e5ae84839e34)
