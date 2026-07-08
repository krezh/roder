# Changelog

## [0.1.22](https://github.com/krezh/roder/compare/0.1.21...0.1.22) (2026-07-08)


### Styles

* modern visual refresh ([af45080](https://github.com/krezh/roder/commit/af45080e2a33200cfd695772254eb7bfd4f2353f))

## [0.1.21](https://github.com/krezh/roder/compare/0.1.20...0.1.21) (2026-07-03)


### Features

* **flux,logs:** add Force/Reset/Reconcile-with-source actions; smarter log streaming ([364aa2b](https://github.com/krezh/roder/commit/364aa2b70669597680ecab58014769b2f5f1d9fe))
* **tree:** add Resource Tree — recursive Kustomization/HelmRelease ownership view ([73af29a](https://github.com/krezh/roder/commit/73af29a05a1f85e871c92114634ad7d5d64affd2))
* **tree:** attach split-pane detail view to Resource Tree ([daad0fc](https://github.com/krezh/roder/commit/daad0fc0a3e98ec89bfe711c2f104b53c6ff5cf4))
* **ui:** add mobile UI with breakpoint-based shell, card lists, and action sheet ([e941da4](https://github.com/krezh/roder/commit/e941da40bb04f75ab87058f9221ee4345738f55f))
* **ui:** add toast notifications for topbar actions, bulk row actions, debug shell, and clipboard copy ([7bd8f93](https://github.com/krezh/roder/commit/7bd8f93e66c80cb1d1cc1d92f294a9c8fbc9ba35))
* **ui:** cache topbar indicators across refresh ([c0e5ce2](https://github.com/krezh/roder/commit/c0e5ce29bb44d9662251bd3bb51cc02b09d3ec07))
* **ui:** make Flux Suspend/Resume context-aware ([3cff7fe](https://github.com/krezh/roder/commit/3cff7fe8a2db2daff9cab68ae8d555c67d45aaba))


### Bug Fixes

* **cargo:** update rust crate rand (0.10.1 ➔ 0.10.2) ([#73](https://github.com/krezh/roder/issues/73)) ([7d549c5](https://github.com/krezh/roder/commit/7d549c546c72743fdcc1e38f8c99bd755d622f36))
* **cargo:** update rust crate time (0.3.51 ➔ 0.3.52) ([#66](https://github.com/krezh/roder/issues/66)) ([4b0c42f](https://github.com/krezh/roder/commit/4b0c42fbb32eb437d14c09d15929e78a2ad39057))
* **cargo:** update rust crate time (0.3.52 ➔ 0.3.53) ([#69](https://github.com/krezh/roder/issues/69)) ([fccc3ac](https://github.com/krezh/roder/commit/fccc3acebc54b2c92ca650032a94437207e06a03))
* **container:** update image ghcr.io/rust-lang/rust (1.96.0 ➔ 1.96.1) ([#68](https://github.com/krezh/roder/issues/68)) ([79f1d0b](https://github.com/krezh/roder/commit/79f1d0bd1b1105f763ee49d272515f3f692ca3f5))
* **container:** update image ghcr.io/rust-lang/rust (58fe975 ➔ 1f0dbad) ([#71](https://github.com/krezh/roder/issues/71)) ([ecaf488](https://github.com/krezh/roder/commit/ecaf4889c260484bf4a57fd53a1145608e7bd1de))
* **ui:** fix the row jitter ([a8cef09](https://github.com/krezh/roder/commit/a8cef09cd0bf4ad83e64591fc19de801d0d4e802))


### Code Refactoring

* consolidate flux reconcile with force/reset parameters ([eb3167c](https://github.com/krezh/roder/commit/eb3167c2b42e9d719787601d42e3ebf14a84bfbe))
* **k8s,ui:** split backend.rs and format.rs into cohesive submodules ([84d49f6](https://github.com/krezh/roder/commit/84d49f614ef10169eee324b0217bdc375b37937a))
* **k8s:** simplify projection and informer helpers ([4fc68c7](https://github.com/krezh/roder/commit/4fc68c712ff6e1258374e7a549e22e207ef088ac))
* remove badge icons and reorganize topbar layout ([dc7c9b3](https://github.com/krezh/roder/commit/dc7c9b397ddce4ee8e2cafa294095e0a17760414))
* **ui:** remove per-kind error/warn status dot from sidebar ([173012e](https://github.com/krezh/roder/commit/173012e04227f14ce13495fbadc401e7a3a9384f))

## [0.1.20](https://github.com/krezh/roder/compare/0.1.19...0.1.20) (2026-06-28)


### Features

* **alertmanager:** support direct HTTP URLs via reqwest ([acb2a96](https://github.com/krezh/roder/commit/acb2a96de06ce97d18b8af5ec9b5ee0cfbf67445))
* **cargo:** update rust crate aes-gcm (0.10.3 ➔ 0.11.0) ([#64](https://github.com/krezh/roder/issues/64)) ([1f39581](https://github.com/krezh/roder/commit/1f39581a0842fb7bb1aee1104b482ca00ab07736))


### Bug Fixes

* **cargo:** update rust crate arc-swap (1.9.1 ➔ 1.9.2) ([#63](https://github.com/krezh/roder/issues/63)) ([bcc2ba7](https://github.com/krezh/roder/commit/bcc2ba79b2acd5af86c8602f9511a3aa0b474d9e))

## [0.1.19](https://github.com/krezh/roder/compare/0.1.18...0.1.19) (2026-06-26)


### Features

* **alertmanager:** add AlertManager integration with alerts panel UI ([49068f4](https://github.com/krezh/roder/commit/49068f47fdd48fa988571d7bb956196292c63cf3))
* Flux failing badge in topbar ([d6851ae](https://github.com/krezh/roder/commit/d6851ae39605dc9d6fd31b273bcee61daa106065))
* **hooks:** coalesce SSE event bursts to reduce reactive recomputes ([85dcfaa](https://github.com/krezh/roder/commit/85dcfaaf9d6399956a8e3286d2b2a1f7bc79e936))
* log parsing for Python/syslog, sidebar favorites + error indicators ([f2c0e6b](https://github.com/krezh/roder/commit/f2c0e6b17a4394aa708aab2be516300db6321ad9))
* pod exec terminal + three k8s backend bug fixes ([9c34fd5](https://github.com/krezh/roder/commit/9c34fd540a37b73d21f2299bae41908adf2d2f54))
* sweep button to sanitize dead pods and finished jobs ([bd55a85](https://github.com/krezh/roder/commit/bd55a8575e7b88b61406bf901b3bcdec0e450f01))
* **ui:** detail drawer improvements ([e1d7096](https://github.com/krezh/roder/commit/e1d70964bbb1dbfce9e3fa3582250409b94af6dd))


### Bug Fixes

* **cargo:** update rust crate leptos (0.8.19 ➔ 0.8.20) ([#60](https://github.com/krezh/roder/issues/60)) ([6c27407](https://github.com/krezh/roder/commit/6c27407427edab6fdb3afbb9e55b511107414c81))
* **cargo:** update rust crate leptos_axum (0.8.9 ➔ 0.8.10) ([#61](https://github.com/krezh/roder/issues/61)) ([e0e3c87](https://github.com/krezh/roder/commit/e0e3c87a35a68739ee6e79354a5d58fa779f71cc))
* **cargo:** update rust crate leptos_router (0.8.13 ➔ 0.8.14) ([#62](https://github.com/krezh/roder/issues/62)) ([759dc6e](https://github.com/krezh/roder/commit/759dc6ec0e711ef5469cd1442ecef77beedc69de))
* **container:** update image ghcr.io/rust-lang/rust (c681116 ➔ 6df234c) ([#59](https://github.com/krezh/roder/issues/59)) ([8a0ac16](https://github.com/krezh/roder/commit/8a0ac1677ecc4f672958b9052eae7c01330b3532))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#58](https://github.com/krezh/roder/issues/58)) ([db6eeeb](https://github.com/krezh/roder/commit/db6eeebcab421d7250b40a97ae48eccf204cfeff))

## [0.1.18](https://github.com/krezh/roder/compare/0.1.17...0.1.18) (2026-06-22)


### Features

* fuzzy highlight in namespace palette, standardize with kind palette ([552e910](https://github.com/krezh/roder/commit/552e910e48608472a2035699fab62d8daf4c326a))


### Bug Fixes

* **cargo:** update rust crate rustls (0.23.40 ➔ 0.23.41) ([#55](https://github.com/krezh/roder/issues/55)) ([b0a7b17](https://github.com/krezh/roder/commit/b0a7b17e1d6f5488d06cf5dc3425b5f20651c566))
* **cargo:** update rust crate time (0.3.49 ➔ 0.3.51) ([#54](https://github.com/krezh/roder/issues/54)) ([d6c3c90](https://github.com/krezh/roder/commit/d6c3c90cd9152c242ebc80d1faf3029ff1755eeb))
* **container:** update image ghcr.io/rust-lang/rust (4fd8406 ➔ c681116) ([#50](https://github.com/krezh/roder/issues/50)) ([0741048](https://github.com/krezh/roder/commit/07410487b5a25e35ce2f3f21d59e26d1a020ea49))
* preserve extra structured log fields; palette focus on open ([e1f50db](https://github.com/krezh/roder/commit/e1f50dbd7902d14112eb851246f19eba09e67fd1))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#56](https://github.com/krezh/roder/issues/56)) ([b05e4f6](https://github.com/krezh/roder/commit/b05e4f6d8485bda67b7d6d4145d54ffadc6ee3a9))


### Continuous Integration

* enable automerge ([d9a2334](https://github.com/krezh/roder/commit/d9a23345207b3f0100fdfce7c2e6aa7b7c4f8cbb))

## [0.1.17](https://github.com/krezh/roder/compare/0.1.16...0.1.17) (2026-06-17)


### Features

* **cargo:** update rust crate tower-http (0.6.11 ➔ 0.7.0) ([#43](https://github.com/krezh/roder/issues/43)) ([8b729fc](https://github.com/krezh/roder/commit/8b729fc8eaf485c9ce02510312b4392efb34e353))
* Refactor ([#48](https://github.com/krezh/roder/issues/48)) ([5c03e2a](https://github.com/krezh/roder/commit/5c03e2a6c0b8bd1e600ae8e4b01553d581a90572))


### Bug Fixes

* **renovate:** group kube and k8s-openapi ([39c273e](https://github.com/krezh/roder/commit/39c273e983a1c9b8ca7981f45790b21ff7060dab))
* **renovate:** group kube and k8s-openapi updates into one PR ([5751cd3](https://github.com/krezh/roder/commit/5751cd366fb7705f3ffd22ea895af75990f324f0))


### Miscellaneous Chores

* **renovate:** add minimumGroupSize ([98cadd0](https://github.com/krezh/roder/commit/98cadd0fdd8c7276f3b667b1c6e60fc20e6cbab4))

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
