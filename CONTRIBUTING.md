# Contributing

Bug reports, fixes and features are welcome.

## Contribution terms

Retrocrunch is source-available under the PolyForm Shield License 1.0.0 (see
[LICENSE.md](LICENSE.md)), and ready-built releases are sold to fund development. So that
contributions can ship in those builds, by submitting a contribution (a pull request,
patch or anything else you send for inclusion) you agree that:

- you wrote it, or otherwise have the right to submit it;
- you license it to Burak Bayram under the MIT No Attribution licence, so it can be
  distributed under the project's licence and in paid builds.

Don't submit code copied from projects under other licences (GPL in particular) or
third-party models and textures. `test_assets/` is git-ignored for that reason.

## Before you open a pull request

```powershell
cargo test
cargo clippy --all-targets
```

Add a line under `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md) for anything users notice.
If you add or upgrade a dependency, regenerate the third-party notices (see "Licence" in
the README).
