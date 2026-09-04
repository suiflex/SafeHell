# Changelog

## [0.4.0](https://github.com/suiflex/SafeHell/compare/v0.3.0...v0.4.0) (2026-09-04)


### ⚠ BREAKING CHANGES

* the MIT option is withdrawn. Anyone who took SafeHell under MIT must move to Apache-2.0 to keep using releases from this version on.

### Features

* license SafeHell under Apache-2.0 only ([b34ed94](https://github.com/suiflex/SafeHell/commit/b34ed9411811e8577f62286c4b25c24d17ca1eb6))

## [0.3.0](https://github.com/suiflex/SafeHell/compare/v0.2.0...v0.3.0) (2026-09-03)


### Features

* accept an agent list in integrate install ([ca957c2](https://github.com/suiflex/SafeHell/commit/ca957c2d644ace515f44ad6ec4db4278a2b1b0d0))
* draw the brand mark above the install picker ([dc5c881](https://github.com/suiflex/SafeHell/commit/dc5c881306ef83dbceeb037c22048e7ad135a698))
* register the shll MCP server with cursor opencode and antigravity ([319daaf](https://github.com/suiflex/SafeHell/commit/319daaf4c36f4d1f35d5174a247fb90551a5b08b))
* replace integrate install with install across every agent ([25a308b](https://github.com/suiflex/SafeHell/commit/25a308b04644afd70ceebafbfdb7aeffe123361a))


### Bug Fixes

* derive the windows vscode profile path from the home argument ([5f1ce4c](https://github.com/suiflex/SafeHell/commit/5f1ce4cf48d5d50a8b1f28d0bb7a8393fcb9e856))
* keep the symlink guard test honest on windows ([d142916](https://github.com/suiflex/SafeHell/commit/d142916e376c56884efca7968b4777d2a477ea56))
* let npm reach the OIDC exchange ([4894043](https://github.com/suiflex/SafeHell/commit/4894043683de5063569e4e37a2b6a9592cd5ed07))
* publish npm with setup-node v7 ([0a4d9cd](https://github.com/suiflex/SafeHell/commit/0a4d9cdb54c3d7456bf402f0513dfa1245ff9434))

## [0.2.0](https://github.com/suiflex/SafeHell/compare/v0.1.0...v0.2.0) (2026-08-30)


### ⚠ BREAKING CHANGES

* renames the project config file, keyring service, and data directory; existing vaults must be recreated with `safehell setup`.

### Features

* add brand assets ([b0ff4bf](https://github.com/suiflex/SafeHell/commit/b0ff4bfa994783f3f924c4f1e5dd3139ba0c2e5e))
* add npm installer package ([8bf0ded](https://github.com/suiflex/SafeHell/commit/8bf0dedcbd4881ac9d3849fe5c7546338ed79666))
* add windows installer script ([712952b](https://github.com/suiflex/SafeHell/commit/712952bcc665fd99bb7f5ef06f5ffec465efcb18))
* harden the logo to match the name ([9ead8bb](https://github.com/suiflex/SafeHell/commit/9ead8bbf91e0a642f6907d055fc15e9d0f2bb67f))
* redraw the logo around the tool's actual job ([79a9c63](https://github.com/suiflex/SafeHell/commit/79a9c63dd4e219d05088b734f03adf3e14fd2ea5))
* redraw the logo as a locked shell ([033ec58](https://github.com/suiflex/SafeHell/commit/033ec583baf04e0c20d8a7b5351684b9378d4736))
* rename safeshell to safehell ([73a0aab](https://github.com/suiflex/SafeHell/commit/73a0aab14e36a0e5009aed511470e216f7c4bc1a))


### Bug Fixes

* keep breaking changes on 0.x off 1.0.0 ([39810ad](https://github.com/suiflex/SafeHell/commit/39810ad04a7dd83adfb1959dd663abcf1da24cb6))
* move off the yanked chacha20 release ([214aec8](https://github.com/suiflex/SafeHell/commit/214aec8f79e1287d4c222cd5219cfc4efb5d7183))
* pin the initial version so the first release is 0.2.0 ([fe4660a](https://github.com/suiflex/SafeHell/commit/fe4660ab8dc40575fcd91f44cfcf1dc7412d4821))


### Miscellaneous Chores

* release as 0.2.0 ([9b33c1e](https://github.com/suiflex/SafeHell/commit/9b33c1ec5237fe24bc76af6906d7008955c10195))
