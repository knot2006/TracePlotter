# blaze-live source

Copied without source changes from `indexable-inc/blaze`, revision
`0c0ca43feb462186ece3e46ad40cd1f8ac4b6270`, `crates/blaze-live`, on 28 October 2026.
The upstream MIT and Apache licences and NOTICE are included.

The renamed crate was not available on crates.io when integrated. A Cargo Git dependency
also scans upstream's intentionally invalid Cargo fixtures and prints misleading errors.
Vendoring this dependency keeps normal builds independent of that compiler repository.
Only the default-off `dev-live` feature links this code. Runtime patching starts only in
debug demo sessions. Update from a reviewed upstream revision, replacing this directory
and the licence files together, then repeat the live-edit and normal Cargo checks.
