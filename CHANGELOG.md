# Changelog

All notable changes are recorded here by release-please from Conventional Commits.

## [0.2.0](https://github.com/discordy-voics/voice/compare/voice-v0.1.0...voice-v0.2.0) (2026-10-19)


### Features

* add packaging, docs and CI for voice-signaling, voice-sfu and regionctl ([75e745c](https://github.com/discordy-voics/voice/commit/75e745c1cf7f6a38ead7bda96e1e00923a6bd912))
* **regionctl:** add region control CLI ([9b6f413](https://github.com/discordy-voics/voice/commit/9b6f413110346ae7372ded17653a462fa652a9cb))
* **regionctl:** drain and undrain live nodes through the gateway ([96c60c7](https://github.com/discordy-voics/voice/commit/96c60c7e13268813ae5c0deaed8f6c09523c8cc2))
* **sfu:** add application use cases and ports ([ae9d65a](https://github.com/discordy-voics/voice/commit/ae9d65ae4331b5f8233a702784c358429c76fea1))
* **sfu:** add domain rules for layers, speaking, rooms and sessions ([b57d5fd](https://github.com/discordy-voics/voice/commit/b57d5fdef8bb33ae86b03ccea0df1194687b629a))
* **sfu:** add NDJSON control server and sfu binary ([492c7b4](https://github.com/discordy-voics/voice/commit/492c7b46d0d7457b0c120f188a804198690db5e9))
* **sfu:** add RTP sequence and timestamp rewriter ([b540a97](https://github.com/discordy-voics/voice/commit/b540a97cbcbaa46582e1b118387afff61ecafbdf))
* **sfu:** add str0m media engine with UDP mux and two-client audio test ([2ed584e](https://github.com/discordy-voics/voice/commit/2ed584eb7cd20fb367c34c52d6c057a1cec6c1bb))
* **signaling:** add Elixir voice signaling service ([d9eac61](https://github.com/discordy-voics/voice/commit/d9eac61669a7248c17907c8fd491cb49d51c426f))
* **voice:** forward a screen share as its own stream beside the camera ([02bd7f3](https://github.com/discordy-voics/voice/commit/02bd7f38fec531a2595137aae26f195ed0087372))


### Bug Fixes

* **sfu:** start outbound rewritten streams with a zero roll-over counter ([6ca4f2e](https://github.com/discordy-voics/voice/commit/6ca4f2e2ac06393400d25ddc1a5faea280d18e56))
* **signaling:** use the SSRCs from the client's offer ([26bd581](https://github.com/discordy-voics/voice/commit/26bd5816aea59a60ea4483f12cff2b41d0febc1f))

## 0.1.0

First release: everything on develop up to this point, deployed to gen-serv.

## 0.1.0

Initial release: Elixir voice signaling, Rust SFU (str0m) and the `regionctl` CLI.
