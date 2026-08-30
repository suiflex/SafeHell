<!--
Title: conventional-commits style, <= 70 chars, no trailing period.
Release Please reads these, so feat: and fix: reach the changelog and drive the
version. Mark a breaking change with ! or a BREAKING CHANGE footer.
e.g. feat: poll a long-running command without holding the broker
e.g. fix: reject a host key that changed since setup
Keep the pull request focused — one logical change is easier to review and revert.
-->

## Summary

<!-- 1-3 bullets on the WHY: the problem this solves or the need it fills. -->

-

## Changes

<!-- What actually changed, grouped by area (src / scripts / npm / packaging / ci / docs). -->

-

## Test plan

<!-- Check what you ran. Leave unchecked what still needs doing, and say so. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --all-targets`
- [ ] `sh tests/install_sh_test.sh` (installer changes)
- [ ] `node npm/install.js --selftest` (npm package changes)
- [ ] `sh packaging/test_render.sh` (formula or manifest changes)
- [ ] Manual verification steps (describe them):

## Security

<!--
Delete this section for a docs-only or CI-only change.

SafeHell brokers credentials and gates remote commands, so answer these if the
change touches src/: does anything new run without approval, can any new value
reach agent-facing output unredacted, and does any secret move outside the
vault or the OS keyring?
-->

## Checks that break quietly

<!-- Tick only the ones this pull request touches. -->

- [ ] Release asset names still match across `release-build.yml`, `scripts/install.sh`, `tests/install_sh_test.sh`, and `npm/install.js`
- [ ] Every download path still verifies against `SHA256SUMS`
- [ ] `.github/labeler.yml` still agrees with the `paths` filters in `.github/workflows/ci-*.yml`

## Notes for reviewers

<!-- Optional: trade-offs, follow-ups, anything risky. -->
