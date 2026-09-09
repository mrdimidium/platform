# Dimidium Labs platform

Shared Rust crates for web services and reusable `mise` tasks for building,
publishing, and validating projects.

## Web service

- [`dimidiumlabs-ui`](crates/ui) provides the shared design system: fonts,
  assets, design tokens, UI components, and the ordered transport-agnostic
  `AssetsCatalog`.
- [`dimidiumlabs-ui-build`](crates/ui-build) is a build-script tool for global
  styles, CSS Modules, classic JavaScript or TypeScript component scripts, and
  files under `src/assets`. Static files are copied unchanged into build output;
  one generated asset array embeds every resource with its logical name, cache
  policy, bytes, and build-time SHA-384 integrity. Its `build(id, sources,
  assets)` API takes package-local paths below `src` instead of assuming a crate
  layout. Compiled CSS and JavaScript also receive a complete 16-hex xxHash64
  filename fingerprint.
- [`dimidiumlabs-server`](crates/server) serves that catalog through Axum and
  applies CSP, integrity-based strong ETags, conditional request, HEAD, and cache policy.
  Its `service` module provides composable Tower admission, client-IP, host,
  rate-limit, drain, HTML, asset, HSTS, body, and redirect primitives. Top-level
  TLS and Hyper transport modules provide connection-level infrastructure.
  Root resources use their conventional paths; other assets are served from
  `/-/assets/`.

Services keep their pages and component-specific assets in their own crate and
compose `AssetsCatalog::new().with(FOUNDATION).with(APPLICATION)` once for the
HTML document, policy layers, and serving adapter.

## Scripts

Executable tasks live in [`tasks/`](tasks). Consuming projects include this
directory with
[mise remote Git includes](https://mise.jdx.dev/tasks/task-configuration.html#remote-git-includes)
and pin the repository to a commit SHA. Task metadata installs pinned tools on
demand, while each task's explicit `--bootstrap` mode installs only its own
APT or DNF dependencies. Project toolchains remain in the consuming project's
`mise.toml`.

Run `mise run <task> -- --help` for the complete command-line interface.

### `package`

Builds only the requested nFPM packages (`deb`, `rpm`, `apk`) or portable
archives (`tar.gz`, `zip`). The consuming project owns its nFPM configuration
and staged files. DEB and RPM packages can be signed with the OpenPGP
environment variables; signed builds require `SOURCE_DATE_EPOCH`, which is
applied to every GPG and RPM signing step. APK supports `PACKAGE_KEY_VERSION`
and APK signing keys. nFPM receives only a small allowlisted environment; raw
keys, passphrases, and provider-specific CI variables are not forwarded.
`--bootstrap` installs the native dependencies for exactly the requested
formats and exits without building an artifact.

```console
mise run package -- --bootstrap --update deb rpm apk zip
mise run package -- --version VERSION --arch ARCH --output DIR deb rpm apk
mise run package -- --archive-root DIR --archive-name NAME --output DIR tar.gz zip
```

### `publish`

Publishes selected package formats to the service and channel under
`https://pkg.dimidiumlabs.io/<service>/`. Existing payloads are retained, and an
S3 lock serializes repository metadata updates.

```console
mise run publish -- --bootstrap --update deb rpm apk
mise run publish -- --service SERVICE --channel CHANNEL --input DIR deb rpm apk
```

Storage uses `S3_BUCKET`, `S3_ENDPOINT`, `S3_PUBLIC_URL`, `S3_ACCESS_KEY_ID`,
and `S3_SECRET_ACCESS_KEY`. Signing uses `PACKAGE_KEY_VERSION`, the `GPG_*`
variables, and `APK_PRIVATE_KEY`. Signing subprocesses receive an allowlisted
environment without storage or CI-provider credentials.

### `check`

Runs contributor/signoff and licensing policy. The default `all` scope also
runs Rust formatting, ShellCheck, Clippy, tests, optional project scripts, and
LCOV coverage. `policy` is available to projects with a different build stack.

```console
mise run check
mise run check -- policy
mise run check -- --script tests/authorized-keys.sh
```

### `release-context`

Resolves the Cargo version, Git revision and commit timestamp, release channel,
and package version into a shell environment file. CI adapters pass source
metadata explicitly; the task does not read provider-specific environment
variables. `--publish` validates only that the source ref is publishable;
package signing and storage credentials are validated by `package` and
`publish`. Provider-specific authentication remains in the calling adapter.

```console
mise run release-context -- \
  --package SERVICE --source-ref refs/heads/main --revision SHA \
  --build-number 42 --publish --output release.env
```

### `container`

Builds one or more tagged OCI images with Docker Buildx. Authentication and
release policy stay with the caller; use `--push` or `--load` to export the
result.

Cache backends are explicit `--cache-from` and `--cache-to` values rather than
being inferred from a CI provider.

```console
mise run container -- --context . --file Dockerfile --platform linux/amd64,linux/arm64 --tag REGISTRY/IMAGE:TAG --push
```

### `chart`

Strictly lints a Helm chart, packages an immutable version, and optionally
pushes it to one or more OCI repositories. Use `--lint-only` when no package is
needed.

```console
mise run chart -- --chart charts/service --version VERSION --app-version VERSION --output dist/charts --push oci://REGISTRY/charts
```

### `github-release`

Finalizes an already published GitHub release: creates or edits the release,
replaces its assets, optionally promotes an immutable image to a mutable alias,
and updates a nightly tag only after every preceding operation succeeds. Named
binaries supplied as `NAME=PATH` are added to releases together with a
deterministic `SHA256SUMS`. The task accepts repository and run metadata as
explicit arguments, does not read runner-specific variables, and assumes `gh`
and Docker authentication were completed by the caller.

```console
mise run github-release -- \
  --repository OWNER/REPOSITORY --channel nightly --tag nightly \
  --title nightly --revision SHA --version VERSION \
  --checks-url URL --asset dist/*.deb dist/*.rpm \
  --binary service-linux-amd64=.container/binary-amd64/service \
  --binary service-linux-arm64=.container/binary-arm64/service \
  --image REGISTRY/IMAGE:VERSION --image-alias REGISTRY/IMAGE:nightly
```

### `licenses-json`

Generates a deterministic JSON bundle of Rust dependency licenses. Repeat
`--target` for all supported targets; `--check` verifies a committed bundle, and
`--offline` uses cached dependency sources. Licenses for bundled non-Rust
resources can be declared in `package.metadata.dimidiumlabs.bundled-licenses`
with an SPDX ID, name, and path relative to `Cargo.toml`.

```console
mise run licenses-json -- --manifest-path Cargo.toml --output licenses.json --target x86_64-unknown-linux-gnu
```

## Contributing

We welcome your contributions, including code, bug reports, ideas, and success
stories.

If you are making a contribution for the first time or from a new email, please
add yourself to the `.mailmap`.

### Signoff

To include your code, we ask that you read and agree to the [CLA](./CLA.md). To
sign, add a `CLA-Version: 1.0` and a `Signed-off-by` trailer to every commit
(`git commit -s --trailer "CLA-Version: 1.0"`). Each commit in a pull request
must carry a valid `Signed-off-by` line matching the commit author. Please use
your real name. We cannot include code from anonymous contributors.

AI agents MUST NOT add Signed-off-by tags. Only humans can legally certify the
Contributor License Agreement.

### AI policy

You may use AI agents when writing code and documentation. AI is not allowed for
media including images, videos, fonts at all. You must fully read, understand,
and cleanup any code generated by the agent. We ask that you disclose the
agent's use and indicate the tool, model, and extent of contribution.

Contributions should include an Assisted-by tag in the following format:
`Assisted-by: AGENT_NAME:MODEL_VERSION [TOOL1] [TOOL2]`, for example:
`Assisted-by: Claude:claude-4.6-opus coccinelle sparse`

Remember, AI agents should make software better, not worse.

## Licensing

Unless noted otherwise, software and configuration are licensed under
Apache-2.0. Documentation is licensed under CC-BY-4.0.
