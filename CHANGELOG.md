# Changelog

## [0.1.16](https://github.com/krezh/roder/compare/0.1.15...0.1.16) (2026-06-14)


### Features

* **server:** load the catalog at startup (pod SA) + single-flight the build ([8d05d10](https://github.com/krezh/roder/commit/8d05d1091217b412266959bc66094c6ce9958f60))


### Bug Fixes

* **style:** split the large main.scss into smaller files ([ebf42f2](https://github.com/krezh/roder/commit/ebf42f238b5053946be84024e032bfa377a71041))
* **test:** isolate memory measurements with a thread-local counter ([ce89af0](https://github.com/krezh/roder/commit/ce89af0efcb9355a0a371648dc2a619d1a3b670e))
* **test:** serialize memory-test measurements ([2b244f5](https://github.com/krezh/roder/commit/2b244f549df9ebef99ff7ee16133307014a2c02a))


### Performance Improvements

* **crd:** skip CRD schemas + page the list — kill the startup OOM ([6dd2742](https://github.com/krezh/roder/commit/6dd274278df2da8bc4ec1065347f8f99a799eb78))
* **metrics:** typed structs + sonic-rs for the metrics/kubelet reads ([6859b35](https://github.com/krezh/roder/commit/6859b357917d694f2cc10acf3591f69732c54629))


### Miscellaneous Chores

* **devenv:** infisical OIDC for dev-auth, heaptrack, ignore recordings ([0b35309](https://github.com/krezh/roder/commit/0b3530997c7fb171c176f9aa372668fb8099c055))

## [0.1.15](https://github.com/krezh/roder/compare/0.1.14...0.1.15) (2026-06-14)


### Features

* **cargo:** update rust crate tikv-jemallocator (0.6.1 ➔ 0.7.0) ([#39](https://github.com/krezh/roder/issues/39)) ([b6242a5](https://github.com/krezh/roder/commit/b6242a59ae55ea77180706299aae6a5b4bcb1e08))
* **crd:** live-refresh catalog & printer columns via a CRD watch ([a51b14d](https://github.com/krezh/roder/commit/a51b14dadf43092a8dd80cea0b87b3b4cd6947f8))


### Bug Fixes

* add container image info to details ([01ce136](https://github.com/krezh/roder/commit/01ce13674feda7e797a4afe65422fbd62cae3be5))

## [0.1.14](https://github.com/krezh/roder/compare/0.1.13...0.1.14) (2026-06-13)


### Features

* **sidebar:** group Rook/Ceph CRDs into one category ([0c43797](https://github.com/krezh/roder/commit/0c43797f6b0a3c762f8050bb6e4b8696f79b5786))
* **web:** fast flat SSE reconnect instead of exponential backoff ([804abb2](https://github.com/krezh/roder/commit/804abb2bbd6608d64db2bc1f807722c02fe57e37))


### Bug Fixes

* **auth:** slim session cookie + redirect to login on 401 ([c70a4d7](https://github.com/krezh/roder/commit/c70a4d76e22342ab212999792ac2949a9d2ba838))
* **auth:** stateless sealed cookies; jemalloc; RODER_ env prefix ([7dc9c16](https://github.com/krezh/roder/commit/7dc9c16b898a295b2af8e407ae3f2da63d2e6062))
* **cargo:** update rust crate time (0.3.48 ➔ 0.3.49) ([#34](https://github.com/krezh/roder/issues/34)) ([de33099](https://github.com/krezh/roder/commit/de3309953c0f0d4d2f5b42205bd2821b5f87a080))
* **informers:** keep self-healing watcher alive; cut memory & OOM ([70692a2](https://github.com/krezh/roder/commit/70692a2dcd0c98ac6d3260e61a90d67c3b030629))

## [0.1.13](https://github.com/krezh/roder/compare/0.1.12...0.1.13) (2026-06-13)


### Bug Fixes

* **cargo:** update wasm-bindgen ([#37](https://github.com/krezh/roder/issues/37)) ([53d72c4](https://github.com/krezh/roder/commit/53d72c47041ab3998c81cb82d1adf8cf013ecaf3))
* **deps:** change images and update wasm-bindgen renovate group ([a2078ea](https://github.com/krezh/roder/commit/a2078eaa93f73ce199f9a4c9ee68ef745a9cb146))

## [0.1.12](https://github.com/krezh/roder/compare/0.1.11...0.1.12) (2026-06-12)


### Bug Fixes

* install rustls crypto provider to fix TLS panic ([209c459](https://github.com/krezh/roder/commit/209c45949cdd24e88e8d2bc9472a1c1c71f9567b))

## [0.1.11](https://github.com/krezh/roder/compare/0.1.10...0.1.11) (2026-06-12)


### Features

* **deps:** upgrade reqwest to 0.13, remove bundled reqwest from openidconnect ([59dbd01](https://github.com/krezh/roder/commit/59dbd012e374a144be93291bae2bd73853ee288c))
* multi-watch SSE, workspace view, pod right-click, log fixes, UI polish ([5541406](https://github.com/krezh/roder/commit/5541406846805aa0743f4d17b01c9a851358bcfe))


### Bug Fixes

* resolve clippy lints (unused_unit, collapsible_match, manual_div_ceil) ([a3e28fd](https://github.com/krezh/roder/commit/a3e28fdaf1a6b1bf2f0581b3c4c2bfef53652691))
* resolve container for multi-container pods in workload log streaming ([7e35c42](https://github.com/krezh/roder/commit/7e35c42c339d97ad4d9f8068ff8f80e854c9f8fe))
* **session:** update rand API for 0.10 compatibility ([e425d84](https://github.com/krezh/roder/commit/e425d845357a9daad71c80d00ac49f38f0d583d3))


### Miscellaneous Chores

* **deps:** bump rand to 0.10.1, time to 0.3.48 ([4533635](https://github.com/krezh/roder/commit/4533635153761f0a5a69f0a197e7a0aa339457cc))

## [0.1.10](https://github.com/krezh/roder/compare/0.1.9...0.1.10) (2026-06-10)


### Features

* **bulk:** Flux reconcile/suspend/resume; right-click multi-select ([28af133](https://github.com/krezh/roder/commit/28af1338a27c634bd46d951f9bc270822495a171))
* **topbar:** connection status dot, node health in cluster usage ([43b9a0e](https://github.com/krezh/roder/commit/43b9a0e8daec536cbc945698279ad3e1b9212231))
* **ux:** scale widget in context menu, Enter/L keyboard shortcuts ([6e18b16](https://github.com/krezh/roder/commit/6e18b1604ec4ded9f2aa6a9a87dd27f93c23d3d6))


### Bug Fixes

* **cargo:** update wasm-bindgen (0.2.122 ➔ 0.2.123) ([#31](https://github.com/krezh/roder/issues/31)) ([cd7692f](https://github.com/krezh/roder/commit/cd7692fbefbb556eee0ed2a7e9867ecf72bdabd8))
* **clippy:** remove explicit auto-derefs in informers.rs ([dabc7a5](https://github.com/krezh/roder/commit/dabc7a53113d5ea7e0346cf95a1d1fad53230de6))
* **sse:** reconnect automatically when connection drops or pod restarts ([0f0080f](https://github.com/krezh/roder/commit/0f0080f18075aa1b1f608a8baace3d5e37d765c6))
* **ux:** address 7 issues found in code review ([985ddc0](https://github.com/krezh/roder/commit/985ddc07f9a4a4abb2750fc187559d989386dfb9))


### Performance Improvements

* **memory:** strip managedFields from cached DynamicObjects ([e0f29f7](https://github.com/krezh/roder/commit/e0f29f7a9b996d69e65a3885c0ae350d13005ded))
* reduce allocations, dedup types, fix column sizing ([f9d1d51](https://github.com/krezh/roder/commit/f9d1d51fbfb2689d97bfe8c4eb5f957bef96e9c9))


### Code Refactoring

* dedup logic, add reconnect to search/palette, clean up ([0bb5371](https://github.com/krezh/roder/commit/0bb5371fa28d8ab49ccfe7113cfa6d5a53e7f30f))

## [0.1.9](https://github.com/krezh/roder/compare/0.1.8...0.1.9) (2026-06-08)


### Bug Fixes

* **cargo:** update rust crate http (1.4.1 ➔ 1.4.2) ([#27](https://github.com/krezh/roder/issues/27)) ([3a326d2](https://github.com/krezh/roder/commit/3a326d22e5a37a4530fa206364e78da68c4c3968))
* **cargo:** update wasm-bindgen (0.2.121 ➔ 0.2.122) ([#29](https://github.com/krezh/roder/issues/29)) ([a832919](https://github.com/krezh/roder/commit/a832919b13bbe88ca04086abb1b626665338b03b))
* **columns:** drop empty status column for CRDs without printer columns ([89666f8](https://github.com/krezh/roder/commit/89666f85905c0d6304928be3786f7d8462f74cca))


### Performance Improvements

* **table:** consolidate per-cell flash Effects into one per-row bitmask ([ae1452f](https://github.com/krezh/roder/commit/ae1452f32777393a627d0a84bbcf6168930f4412))


### Miscellaneous Chores

* **helm:** increase memory limit and request to 400Mi ([894f8f2](https://github.com/krezh/roder/commit/894f8f2209df25678a9ea98f9cb001bb1ecc245a))

## [0.1.8](https://github.com/krezh/roder/compare/0.1.7...0.1.8) (2026-06-08)


### Bug Fixes

* **lint:** resolve clippy warnings in CI ([fb84a58](https://github.com/krezh/roder/commit/fb84a587ef36ef981eda4897bc88f9f2969ca6b7))
* **wasm:** fix scroll cleanup compile error and search improvements ([f338d78](https://github.com/krezh/roder/commit/f338d78cf9748ba44395ba0970c861952e93577b))

## [0.1.7](https://github.com/krezh/roder/compare/0.1.6...0.1.7) (2026-06-08)


### Bug Fixes

* **docker:** bust buildx cache when wasm-bindgen version changes ([ce56373](https://github.com/krezh/roder/commit/ce56373448cbbb1437e454d940eff17fa2c20109))

## [0.1.6](https://github.com/krezh/roder/compare/0.1.5...0.1.6) (2026-06-08)


### Bug Fixes

* **build:** fix build ([bf56013](https://github.com/krezh/roder/commit/bf560137df79862a7da04d677ce1a6d5d6604d34))

## [0.1.5](https://github.com/krezh/roder/compare/0.1.4...0.1.5) (2026-06-08)


### Bug Fixes

* **wasm:** fix wasm version mismatch ([7c96480](https://github.com/krezh/roder/commit/7c96480f9e49b22a04c796c6b9f2bccea9e414fd))

## [0.1.4](https://github.com/krezh/roder/compare/0.1.3...0.1.4) (2026-06-08)


### Bug Fixes

* **ci:** improve ci ([6d6e794](https://github.com/krezh/roder/commit/6d6e7946433395fdb03463ecab0e8cbe5c1330c4))

## [0.1.3](https://github.com/krezh/roder/compare/0.1.2...0.1.3) (2026-06-08)


### Features

* **cargo:** update rust crate gloo-net (0.6.0 ➔ 0.7.0) ([#19](https://github.com/krezh/roder/issues/19)) ([ba1d664](https://github.com/krezh/roder/commit/ba1d66455971be505e951a832101c51d713b0c9d))


### Bug Fixes

* **oauth:** fix oath ([f174b9d](https://github.com/krezh/roder/commit/f174b9d8520f6065f69b5546543d02f3c3267e1a))

## [0.1.2](https://github.com/krezh/roder/compare/0.1.1...0.1.2) (2026-06-07)


### Bug Fixes

* **cargo:** update rust crate leptos (0.8.14 ➔ 0.8.19) ([#15](https://github.com/krezh/roder/issues/15)) ([4e63686](https://github.com/krezh/roder/commit/4e636861b611e3bea08edc8dbc78001ea8d68897))
* **cargo:** update rust crate leptos_axum (0.8.7 ➔ 0.8.9) ([#16](https://github.com/krezh/roder/issues/16)) ([be26cef](https://github.com/krezh/roder/commit/be26ceffcfd094e9665640b58bab8e272b7d9e8e))
* fix envFrom ([6aefdfa](https://github.com/krezh/roder/commit/6aefdfa4bec3b1b51f4f76d59ff418afdbc43f24))

## [0.1.1](https://github.com/krezh/roder/compare/0.1.0...0.1.1) (2026-06-07)


### Bug Fixes

* **cargo:** update rust-wasm-bindgen monorepo ([#7](https://github.com/krezh/roder/issues/7)) ([df1e80a](https://github.com/krezh/roder/commit/df1e80a6dd4266c659bd2a7251ee3465185946bf))


### Miscellaneous Chores

* **deps:** add .renovaterc.json5 ([#1](https://github.com/krezh/roder/issues/1)) ([22887d5](https://github.com/krezh/roder/commit/22887d5874d2e21c06b821937f9750640c0117b3))
