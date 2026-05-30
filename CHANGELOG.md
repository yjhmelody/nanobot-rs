# Changelog

All notable changes to this project are documented in this file.

<!-- changelog-entries -->

## [v0.0.12] - 2026-05-31

### Added
- feat(cli): add global --config flag for custom config path ([`ffa4fb2`](https://github.com/yjhmelody/nanobot-rs/commit/ffa4fb2966019e8ec6abb590816cf62bd7b81df0))
- refactor: multi-instance channel support (RFC-0008) + agent cancel improvements (RFC-0006) ([`b4b4c1d`](https://github.com/yjhmelody/nanobot-rs/commit/b4b4c1d7b2a316e37c6c96ef5906165b2064cbc3))

### Fixed
- fix(feishu): prioritize user_id > union_id > open_id for stable sender id ([`5a4e433`](https://github.com/yjhmelody/nanobot-rs/commit/5a4e43394a6dad4e02b28d0f1e47c6e4984e692d))
- fix(feishu): use union_id as stable sender identifier; reduce progress log noise ([`5624f82`](https://github.com/yjhmelody/nanobot-rs/commit/5624f827772adb41aad110451ea93418021ca1cb))


## [v0.0.11] - 2026-05-31

### Added
- feat(agent): implement /cancel command with message queue clearing ([`aa0509e`](https://github.com/yjhmelody/nanobot-rs/commit/aa0509ebae6da52c70a294c69116dc97528e199b))

### Changed
- refactor(agent): remove noisy intermediate tool call hints ([`d77191f`](https://github.com/yjhmelody/nanobot-rs/commit/d77191f59fbf0278ad3b8b55ac6255d23dfe9e91))

### Fixed
- fix(provider): capture input_tokens from message_start SSE event ([`657119a`](https://github.com/yjhmelody/nanobot-rs/commit/657119a71336a8f14f1e0d3e8c9b4c88eb732689))
- fix(provider): emit usage update alongside finish reason in message_delta ([`77eca3c`](https://github.com/yjhmelody/nanobot-rs/commit/77eca3cc545185650deb0334ff862995776732f4))
- fix(agent): throttle progress updates only when both time and size thresholds met ([`68b9088`](https://github.com/yjhmelody/nanobot-rs/commit/68b90888623f2ec32ab495e3b553f40705670136))

### Documentation
- docs: add AI agent development pain points guide ([`8fec3a2`](https://github.com/yjhmelody/nanobot-rs/commit/8fec3a2640eccdfd4b76ed0d207b142af76448c5))


## [v0.0.10] - 2026-05-29

### Added
- feat(bus): log outbound messages with type and content preview ([`a63dca9`](https://github.com/yjhmelody/nanobot-rs/commit/a63dca9896bafbb570b3c9e6dcfc801ff83c20b8))
- feat(channels/feishu): batch edits and shard long streaming messages ([`4aceb16`](https://github.com/yjhmelody/nanobot-rs/commit/4aceb16a74d2caa8dc0ae0c8470e27f4ca0451ae))
- feat(bus): log inbound messages with content preview ([`fc56797`](https://github.com/yjhmelody/nanobot-rs/commit/fc56797f53df3c531b550a2f0749945a35be9adc))
- feat(types): add shared UTF-8 truncation utilities ([`81cd896`](https://github.com/yjhmelody/nanobot-rs/commit/81cd8967e7d64945ce8971628deef346a54284e0))
- feat(provider): add ThinkingBlock with signature for extended thinking ([`a5850dd`](https://github.com/yjhmelody/nanobot-rs/commit/a5850ddb33acfbcbdbf543c9e85698562d234feb))

### Changed
- refactor: replace local truncation fns with nanobot-types::text ([`c5e55bf`](https://github.com/yjhmelody/nanobot-rs/commit/c5e55bfeb64c80b53f841537c1f6a2d311bc01fb))
- refactor(provider): strong-type Anthropic API types ([`755b1eb`](https://github.com/yjhmelody/nanobot-rs/commit/755b1ebe2db8dedf4e7274820bd762cc39af9c92))

### Fixed
- fix: prevent agent stalls from hanging shell commands and WS drops ([`24cf713`](https://github.com/yjhmelody/nanobot-rs/commit/24cf713dc45b2f13b6fd9428c80cf05dfcfd55e7))
- fix(provider/openai-compat): pass reasoning_effort in payload ([`e46434f`](https://github.com/yjhmelody/nanobot-rs/commit/e46434f22a7c09d4b00015f4a0bd240050bbfe24))

### Documentation
- docs(changelog): update for v0.0.10 ([`fcb94eb`](https://github.com/yjhmelody/nanobot-rs/commit/fcb94ebbdecb33c041f352e4cfbba74048588e28))


## [v0.0.9] - 2026-05-28

### Added
- feat(feishu): support image send and receive ([`07d14ff`](https://github.com/yjhmelody/nanobot-rs/commit/07d14ff270a62f1e19e074939db8fd7ef6762437))

### Changed
- fix agent loop stall handling ([`85acc36`](https://github.com/yjhmelody/nanobot-rs/commit/85acc36852a37063b3509688fbaa7c91643daa95))

### Documentation
- Fix changelog range handling and enforce tag-version alignment ([`1cf3c11`](https://github.com/yjhmelody/nanobot-rs/commit/1cf3c119e63b71c091ca14ea8be055e5ac12a90d))


## [v0.0.8] - 2026-05-28

### Added
- Add loop-level usage aggregation ([`aab7db6`](https://github.com/yjhmelody/nanobot-rs/commit/aab7db6e1d5f8ebdccc2a978232759ea6dd4729e))

### Documentation
- Enforce changelog check before tag push ([`63cdd05`](https://github.com/yjhmelody/nanobot-rs/commit/63cdd051ac3f5e44846fbf3facb3041df1ffc6d0))


## [v0.0.5] - 2026-05-27

### Added
- Add open-source release files (LICENSE, CONTRIBUTING) ([`78c1c34`](https://github.com/yjhmelody/nanobot-rs/commit/78c1c3483219ad75c1e69176d319d08cc1c07502))

### Changed
- Restrict CI/release to macOS ARM only; restore multi-platform CI ([`c14e17f`](https://github.com/yjhmelody/nanobot-rs/commit/c14e17f7a86c5a68ec62dba3ace1f05cc3662f04))
- Windows CI: build only, skip tests ([`1edd132`](https://github.com/yjhmelody/nanobot-rs/commit/1edd13265f0e4882ab43ccd040ce3cead520904d))
- Skip e2e tests on Windows in CI test job ([`fa9d077`](https://github.com/yjhmelody/nanobot-rs/commit/fa9d07770a2557043ff95560d09b1d76a0bf16ea))
- Downgrade Rust toolchain from 1.95 to 1.93.1 ([`5fdf344`](https://github.com/yjhmelody/nanobot-rs/commit/5fdf34474a072b292c244c4e773ee28c26e90511))
- Split CI into lint/test/e2e jobs; skip lint on Windows ([`fd56cf5`](https://github.com/yjhmelody/nanobot-rs/commit/fd56cf535c5e081af7637c2b751b5de60cdd5095))

### Documentation
- Improve changelog commit links ([`22d4bba`](https://github.com/yjhmelody/nanobot-rs/commit/22d4bbaceb7832829c12e7df912d2a2d74af091f))


## [v0.0.4] - 2026-05-27

### Added
- Add configurable keepRecent and unify project hook gates ([`2150848`](https://github.com/yjhmelody/nanobot-rs/commit/2150848))

### Documentation
- Improve release automation and changelog tooling ([`627fce7`](https://github.com/yjhmelody/nanobot-rs/commit/627fce7))
