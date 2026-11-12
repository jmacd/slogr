# slogr-build

A `build.rs` helper for applications using slogr and OpenTelemetry Weaver.
Weaver resolves and validates registry declarations and standard references.
The helper generates a public Rust `attributes` module with lowercase statics
and its own `#![allow(non_upper_case_globals)]`.

## Application setup

Build the patched Weaver CLI in this repository's `tools/weaver` submodule:

```sh
git submodule update --init tools/weaver
cd tools/weaver/ui
pnpm install --frozen-lockfile
pnpm build
cd ../../..
cargo build --manifest-path tools/weaver/Cargo.toml --locked --bin weaver
export SLOGR_WEAVER="$PWD/tools/weaver/target/debug/weaver"
```

The fork is based on **0.28.0**, with a local fix separating the global
`--project-config` option from `registry generate --config` (template YAML).
Unpatched 0.28.0 conflates these and can report success without generating files.
The helper requires this fix, and does not fall back to ambient configuration.
Put the patched binary on `PATH`, set `SLOGR_WEAVER`, or use `Builder::weaver`.
No tool is downloaded or compiled by the build helper.
Building Weaver from source requires Node.js 24 and the pnpm version pinned in
`tools/weaver/ui/package.json` to generate its embedded UI assets; no browser or
server needs to run.

```toml
[dependencies]
slogr = "0.1"

[build-dependencies]
slogr-build = "0.1"
```

In `build.rs`:

```rust,no_run
fn main() -> Result<(), Box<dyn std::error::Error>> {
    slogr_build::Builder::new("semconv")
        .group("registry.my_app")
        .generate()?;
    Ok(())
}
```

In the application:

```rust,ignore
include!(concat!(env!("OUT_DIR"), "/slogr_attributes.rs"));
use attributes::{my_app_requests, service_name};

slogr::otel_info!(
    "Request received", service_name = "my-app", my_app_requests = 1u64,
);
```

The generated source uses `::slogr`, so keep that dependency name available.
The output can also be included inside another module to avoid an existing
`attributes` module.

## Registry configuration

Use Weaver's groups-based attribute format, with an explicit definition manifest
and a pinned standard dependency in `semconv/manifest.yaml`:

```yaml
file_format: definition_manifest/2.0
name: com.example.my_app
description: Attributes used by my application.
schema_url: https://example.com/my-app/schemas/0.1.0
dependencies:
  - schema_url: https://opentelemetry.io/schemas/1.41.0
    registry_path: https://github.com/open-telemetry/semantic-conventions.git@v1.41.0[model]
```

Declare or reference the application's attributes in `semconv/attributes.yaml`:

```yaml
groups:
  - id: registry.my_app
    type: attribute_group
    brief: Attributes used by my application.
    attributes:
      - ref: service.name
      - id: my_app.requests
        type: int
        brief: Number of requests.
        stability: development
        annotations:
          slogr:
            type: u64
```

**The default Rust type is always `str`.** Weaver's built-in `type` is not used
to infer the Rust type. Non-string declarations require the
`annotations.slogr.type` extension. This also works on a `ref`, so applications
can specify the Rust type of a standard attribute without editing upstream.

Supported explicit types are `str`, `bool`, `i8`, `i16`, `i32`, `i64`, `isize`,
`u8`, `u16`, `u32`, `u64`, `usize`, `f32`, and `f64`. Other types and malformed
extensions are errors. Weaver's schema type is still required for declarations;
keep it consistent with the value you intend to emit if using live-check.
For example, referencing an integer attribute without a slogr override generates
`Attribute<str>`, not `Attribute<i64>`.

Generated string attributes accept any `AsRef<str>` input, such as `String`,
`&String`, `Cow<str>`, or a custom string wrapper, without formatting or copying.
Non-string attributes retain their declared Rust type checks.

The generator takes the resolved wire name and combines `brief` and `note` for
the description. Symbols are lowercase, with punctuation collapsed to
underscores: `http.request.method` becomes `http_request_method`. Rust keywords
use raw identifiers where possible. Identical declarations deduplicate; symbol
collisions and conflicting types or descriptions fail generation.

## Reproducible builds

Select one or more application groups with repeated `.group(...)` calls. Missing
groups fail rather than producing an incomplete registry. Only selected groups
are emitted; importing a standard registry does not generate all its attributes.

The helper watches the registry directory, the Weaver executable when found,
`PATH`, and `SLOGR_WEAVER`. It disables ambient `.weaver.toml` discovery by default;
use `.config("path/to/.weaver.toml")` to load and watch a project configuration
explicitly. This also avoids repeatedly rebuilding because of missing optional
configuration files.

Register local dependencies or policies outside the registry directory with
`.watch("path")`. Remote dependencies should be pinned to
immutable revisions; updating the manifest triggers generation. Weaver may need
network access for remote dependencies; vendor them locally for offline builds.

Generation writes only under `OUT_DIR` (or `.out_dir(...)`). Weaver policies stay
enabled, and failures include subprocess diagnostics. Stale generated code is
never used as a fallback. Build scripts must propagate the returned error.
