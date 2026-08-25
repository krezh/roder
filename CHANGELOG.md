# Changelog

## [0.1.45](https://github.com/krezh/roder/compare/0.1.44...0.1.45) (2026-08-25)


### Features

* **alertmanager:** add authorized alert silencing ([906fe91](https://github.com/krezh/roder/commit/906fe91b238c226036b6a3992fa444c32fd98f60))
* **app:** support browser history navigation ([eaf90d1](https://github.com/krezh/roder/commit/eaf90d1adebcd768e3f446baac9dc71e6e8e6770))
* **dashboard:** redesign cluster overview ([99443fd](https://github.com/krezh/roder/commit/99443fd7c1274294db71132bb6fcaa4e43097675))


### Bug Fixes

* **cargo:** update rustwright digest (af00b37 ➔ 769674b) ([#195](https://github.com/krezh/roder/issues/195)) ([f28ca41](https://github.com/krezh/roder/commit/f28ca4159be1c8ccdca44943f94aef987b51d6e8))
* **cargo:** update rustwright digest (af71449 ➔ eb88543) ([#193](https://github.com/krezh/roder/issues/193)) ([13b11a2](https://github.com/krezh/roder/commit/13b11a241edcb1c3c602103815c66e7b734defcf))
* **cargo:** update rustwright digest (eb88543 ➔ af00b37) ([#194](https://github.com/krezh/roder/issues/194)) ([5f3df6d](https://github.com/krezh/roder/commit/5f3df6dfae1629d6bcbf8655d4bb9cc96dfa10a0))
* **ui:** brighten topbar button text ([7378db1](https://github.com/krezh/roder/commit/7378db1b75af4bbb5478f5bd5543b681f7822874))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#191](https://github.com/krezh/roder/issues/191)) ([a34fcaf](https://github.com/krezh/roder/commit/a34fcaf5bc1b25ff7aa3acc9743fb1f674f288d8))
* **cargo:** lock file maintenance cargo.lock ([#196](https://github.com/krezh/roder/issues/196)) ([58a031a](https://github.com/krezh/roder/commit/58a031aa71e84d2771b2b2cb7604e09875978ee9))

## [0.1.44](https://github.com/krezh/roder/compare/0.1.43...0.1.44) (2026-08-22)


### Features

* **container:** update image ghcr.io/rust-lang/rust (1.97.1 ➔ 1.98.0) ([#187](https://github.com/krezh/roder/issues/187)) ([6384b1f](https://github.com/krezh/roder/commit/6384b1f2fc05178d1fe0bdcebe5e010e53d2d365))


### Bug Fixes

* **cargo:** update rust crate aes-gcm (0.11.0 ➔ 0.11.1) ([#189](https://github.com/krezh/roder/issues/189)) ([7670068](https://github.com/krezh/roder/commit/767006860ce9060bae92606553dc3e0b448edce4))
* **clippy:** box Response errors and add real ResolveError type ([#190](https://github.com/krezh/roder/issues/190)) ([5aa684b](https://github.com/krezh/roder/commit/5aa684b905ba3bca31591ecf95f9c5df7b36e909))
* **container:** update image gcr.io/distroless/cc-debian13 (ed7c407 ➔ e86cf4f) ([#188](https://github.com/krezh/roder/issues/188)) ([dde54af](https://github.com/krezh/roder/commit/dde54afc5f041a97ac71b7634034f70271f8ced2))
* **ha:** set Content-Type on peer-forwarded drain requests ([8acfb0f](https://github.com/krezh/roder/commit/8acfb0f5e9e7e830524194a3bc66c6d69b0b2899))
* **mobile:** stop alert/node badges double-firing, mount missing delete dialog, add drawer scrim ([8cc12eb](https://github.com/krezh/roder/commit/8cc12eb961e472bd0cec03e0e4852f95262a57e0))
* **tooltip:** wrap long text instead of overflowing the bubble ([3655cac](https://github.com/krezh/roder/commit/3655cac150ff112c58488eac9a44dc4246e5a95d))


### Styles

* **fmt:** reformat browser_smoke.rs ([ddb85fd](https://github.com/krezh/roder/commit/ddb85fd52e14bc1fcfa8efb44e61175dcb14093e))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#182](https://github.com/krezh/roder/issues/182)) ([702f17a](https://github.com/krezh/roder/commit/702f17aa24b608c045935eb2dfa02b7d88d16f0d))
* **cargo:** lock file maintenance cargo.lock ([#185](https://github.com/krezh/roder/issues/185)) ([896f70e](https://github.com/krezh/roder/commit/896f70ef444fe67b849175fd311d171dc3a2676a))
* **cargo:** lock file maintenance cargo.lock ([#186](https://github.com/krezh/roder/issues/186)) ([4d024b7](https://github.com/krezh/roder/commit/4d024b73f46613f299e2b671428c039c9c29ef0b))

## [0.1.43](https://github.com/krezh/roder/compare/0.1.42...0.1.43) (2026-08-18)


### Bug Fixes

* **cargo:** update rust crate futures (0.3.33 ➔ 0.3.34) ([#165](https://github.com/krezh/roder/issues/165)) ([91a7ac9](https://github.com/krezh/roder/commit/91a7ac98b882fd3ae20e121f9c75d7ddc28d7d78))
* **cargo:** update rust crate rcgen (0.14.8 ➔ 0.14.9) ([#163](https://github.com/krezh/roder/issues/163)) ([e15db75](https://github.com/krezh/roder/commit/e15db754229d13dcdcd275903716cea669c9c6b3))
* **cargo:** update rust crate thiserror (2.0.19 ➔ 2.0.20) ([#159](https://github.com/krezh/roder/issues/159)) ([c0ef93e](https://github.com/krezh/roder/commit/c0ef93ea54d34b83691904f8bb4c13f70701bca8))
* **cargo:** update rustwright digest (46ae77e ➔ 6eb07e0) ([#170](https://github.com/krezh/roder/issues/170)) ([b2f7f9b](https://github.com/krezh/roder/commit/b2f7f9b75a30ec6eae97f8616b53b9dd8c424bb0))
* **cargo:** update rustwright digest (58be34b ➔ eec542e) ([#173](https://github.com/krezh/roder/issues/173)) ([0e904a1](https://github.com/krezh/roder/commit/0e904a14f26ebd59ec8c3bcd9cbd3d8c4c6488ec))
* **cargo:** update rustwright digest (6eb07e0 ➔ 58be34b) ([#172](https://github.com/krezh/roder/issues/172)) ([61127fc](https://github.com/krezh/roder/commit/61127fc15c833358adf40e694b82557f7135373c))
* **cargo:** update rustwright digest (71d85db ➔ af71449) ([#178](https://github.com/krezh/roder/issues/178)) ([305ac09](https://github.com/krezh/roder/commit/305ac09f16f64874427b773c1ea9946875caba2b))
* **cargo:** update rustwright digest (c9f4307 ➔ 46ae77e) ([#169](https://github.com/krezh/roder/issues/169)) ([a489b9a](https://github.com/krezh/roder/commit/a489b9add88b756218febe0e9062e038b23cd97a))
* **cargo:** update rustwright digest (eec542e ➔ 71d85db) ([#174](https://github.com/krezh/roder/issues/174)) ([b9b57fe](https://github.com/krezh/roder/commit/b9b57fe4957944fc5ae1b20b9aa922800154283c))
* **container:** update image ghcr.io/rust-lang/rust (3382bd2 ➔ b1b3c9c) ([#176](https://github.com/krezh/roder/issues/176)) ([58def9a](https://github.com/krezh/roder/commit/58def9a6145da2b27dec9fc571b7f5c8e43ac593))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#157](https://github.com/krezh/roder/issues/157)) ([6b2b7db](https://github.com/krezh/roder/commit/6b2b7db773676974c82e57684ccbe3df66011356))
* **cargo:** lock file maintenance cargo.lock ([#161](https://github.com/krezh/roder/issues/161)) ([47b2461](https://github.com/krezh/roder/commit/47b246173cb94beec4a522ebdae8d0ad93d92a7d))
* **cargo:** lock file maintenance cargo.lock ([#162](https://github.com/krezh/roder/issues/162)) ([b95426a](https://github.com/krezh/roder/commit/b95426a097cb361135bdcf2f60edce47c99ccb8b))
* **cargo:** lock file maintenance cargo.lock ([#164](https://github.com/krezh/roder/issues/164)) ([2149861](https://github.com/krezh/roder/commit/21498618056f0737964c18fe86c8ab36c8f3e955))
* **cargo:** lock file maintenance cargo.lock ([#166](https://github.com/krezh/roder/issues/166)) ([c6c3b26](https://github.com/krezh/roder/commit/c6c3b264299fccdab16528367c25ea4cb116e11b))
* **cargo:** lock file maintenance cargo.lock ([#168](https://github.com/krezh/roder/issues/168)) ([0226abc](https://github.com/krezh/roder/commit/0226abc28dd3dc3037f0aef965f459f9972fe484))
* **cargo:** lock file maintenance cargo.lock ([#171](https://github.com/krezh/roder/issues/171)) ([0188b1f](https://github.com/krezh/roder/commit/0188b1fdca18182ccd60aeb5dc019425ced7293f))
* **cargo:** lock file maintenance cargo.lock ([#175](https://github.com/krezh/roder/issues/175)) ([540cfd3](https://github.com/krezh/roder/commit/540cfd3c81e204b7f23de028df2f2438338bdfdf))
* **cargo:** lock file maintenance cargo.lock ([#179](https://github.com/krezh/roder/issues/179)) ([7afe1a2](https://github.com/krezh/roder/commit/7afe1a29d238f642b005f9bc833761e1237c17a5))
* **cargo:** lock file maintenance cargo.lock ([#181](https://github.com/krezh/roder/issues/181)) ([ec5d7d8](https://github.com/krezh/roder/commit/ec5d7d87bfce30109e3d526126d8121328266653))

## [0.1.42](https://github.com/krezh/roder/compare/0.1.41...0.1.42) (2026-08-07)


### Features

* **ui:** display time since last pod restart ([5debadd](https://github.com/krezh/roder/commit/5debadd0ff3bd0db4192a8dc0761e40db5bd2e22))


### Bug Fixes

* **docker:** copy hash.txt and set LEPTOS_HASH_FILES so pkg assets resolve ([50a5370](https://github.com/krezh/roder/commit/50a53704bbf96132bb9e36544a6b8cb94a1c5537))


### Continuous Integration

* **e2e:** add headless-Chromium browser smoke test ([#154](https://github.com/krezh/roder/issues/154)) ([1f604f9](https://github.com/krezh/roder/commit/1f604f9e416cef4af1f63ab4452f49cd48ad501c))

## [0.1.41](https://github.com/krezh/roder/compare/0.1.40...0.1.41) (2026-08-07)


### Bug Fixes

* **container:** update image ghcr.io/rust-lang/rust (1bcff4b ➔ 3382bd2) ([#148](https://github.com/krezh/roder/issues/148)) ([5a6a615](https://github.com/krezh/roder/commit/5a6a615056cd8e3d57bbb5013ae35fbfa4190c75))
* resolve hashed wasm filename from hash.txt manifest ([7b9ad38](https://github.com/krezh/roder/commit/7b9ad3828ede4f86193972268a851dc5f7a5f2f4))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#146](https://github.com/krezh/roder/issues/146)) ([753f55e](https://github.com/krezh/roder/commit/753f55e7e759d4d2fc17bc76d8bc1a3d45e6502f))
* **cargo:** lock file maintenance cargo.lock ([#150](https://github.com/krezh/roder/issues/150)) ([0cd6a59](https://github.com/krezh/roder/commit/0cd6a59507c71519dbe82bd5ae4a39978fee59c5))

## [0.1.40](https://github.com/krezh/roder/compare/0.1.39...0.1.40) (2026-08-05)


### Features

* **cargo:** update rust crate http (1.4.2 ➔ 1.5.0) ([#136](https://github.com/krezh/roder/issues/136)) ([fccf039](https://github.com/krezh/roder/commit/fccf039bb8a8d5b470b8f49d26e7efaebe449129))
* **table:** truncate the single widest overflowing column ([9613d8a](https://github.com/krezh/roder/commit/9613d8aae833e34efd9610be41f1db54439bb6ed))
* **tooltip:** follow the cursor and cover every hover hint app-wide ([6e2c35d](https://github.com/krezh/roder/commit/6e2c35de87e8d1914e5a9a12abb21e1ab092310e))


### Bug Fixes

* **cache:** content-hash pkg assets so css updates aren't cached for a year ([7dc8f98](https://github.com/krezh/roder/commit/7dc8f98ebc6bf6c8668f10074f67d37c8160b30c))
* **cargo:** update rust crate base64 (0.23.0 ➔ 0.23.1) ([#144](https://github.com/krezh/roder/issues/144)) ([aa736c0](https://github.com/krezh/roder/commit/aa736c0a60934ee4337022cab196e959efc173bd))
* **cargo:** update rust crate rustls (0.23.42 ➔ 0.23.43) ([#137](https://github.com/krezh/roder/issues/137)) ([e32cc65](https://github.com/krezh/roder/commit/e32cc65ab1178e4787023bfb525ba236c8543ac0))
* **cargo:** update rust crate time (0.3.54 ➔ 0.3.55) ([#140](https://github.com/krezh/roder/issues/140)) ([a89a81c](https://github.com/krezh/roder/commit/a89a81cdca6f21fefbc1e8e173794b2145bb0185))
* **k8s:** join multi-value printer columns with newlines for list tooltips ([bb573f9](https://github.com/krezh/roder/commit/bb573f9e2c1ad2186a66e618837d4e3d87bfe598))
* **talos:** stop node detail table columns overlapping on resize ([f540f52](https://github.com/krezh/roder/commit/f540f52684614d83164cca66d1413232c07db416))


### Styles

* **ui:** make the sidebar's minimal scrollbar the app-wide default ([6e80a88](https://github.com/krezh/roder/commit/6e80a883afa45e7bf5b6a8449f7e7f44dd9daef1))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#131](https://github.com/krezh/roder/issues/131)) ([18f6f77](https://github.com/krezh/roder/commit/18f6f77b5b9fda9ee7cfdaf8784372ccf050d4c5))
* **cargo:** lock file maintenance cargo.lock ([#134](https://github.com/krezh/roder/issues/134)) ([8d591bb](https://github.com/krezh/roder/commit/8d591bb59f04202e0b2fc1deaa2678114bcfdd97))
* **cargo:** lock file maintenance cargo.lock ([#138](https://github.com/krezh/roder/issues/138)) ([5f0e960](https://github.com/krezh/roder/commit/5f0e960d6273c9863e00d1184446fc92da295a7f))
* **cargo:** lock file maintenance cargo.lock ([#139](https://github.com/krezh/roder/issues/139)) ([9383873](https://github.com/krezh/roder/commit/938387378a03fc7b4a5752f652433273e71ef796))
* **cargo:** lock file maintenance cargo.lock ([#141](https://github.com/krezh/roder/issues/141)) ([a748885](https://github.com/krezh/roder/commit/a748885885fe1d4ac72162ca978c2af2364cdad7))
* **cargo:** lock file maintenance cargo.lock ([#142](https://github.com/krezh/roder/issues/142)) ([c7b389f](https://github.com/krezh/roder/commit/c7b389fbb28d30045532eb4fdf018c1fd78d6211))
* **cargo:** lock file maintenance cargo.lock ([#143](https://github.com/krezh/roder/issues/143)) ([656c378](https://github.com/krezh/roder/commit/656c378709481625ba93faa1b0520791011a6c1e))
* **cargo:** lock file maintenance cargo.lock ([#145](https://github.com/krezh/roder/issues/145)) ([55ba828](https://github.com/krezh/roder/commit/55ba8281a98e09a133571b0d54f0cca4b7e74d8e))

## [0.1.39](https://github.com/krezh/roder/compare/0.1.38...0.1.39) (2026-07-27)


### Features

* **client:** live-render timestamp columns on the age tick ([e91b37b](https://github.com/krezh/roder/commit/e91b37bb6b872041cfe56ece701a90e469854f46))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#129](https://github.com/krezh/roder/issues/129)) ([568f0f9](https://github.com/krezh/roder/commit/568f0f9e5f1a0fd0c7617dc067d283ad71123cb2))

## [0.1.38](https://github.com/krezh/roder/compare/0.1.37...0.1.38) (2026-07-26)


### Bug Fixes

* **helm:** stop filtering service endpoints by HA protocol ([8c68bbd](https://github.com/krezh/roder/commit/8c68bbd48b36a86b0e08b36cb81fce23a43c50cd))
* update reqwest ([22adace](https://github.com/krezh/roder/commit/22adace9b60a41be98d9a08988972536cdbba291))

## [0.1.37](https://github.com/krezh/roder/compare/0.1.36...0.1.37) (2026-07-26)


### Features

* **cargo:** Update Rust crate serial_test (3.5.0 ➔ 4.0.1) ([#123](https://github.com/krezh/roder/issues/123)) ([d3b613a](https://github.com/krezh/roder/commit/d3b613a289bf92942ec6a65962079554fe7542d9))
* **ha:** self-signed mTLS between roder peers ([bb1a390](https://github.com/krezh/roder/commit/bb1a390fd59ff673c1d24dfec257739842831b64))
* make status indicators clickable as filter ([bb0d781](https://github.com/krezh/roder/commit/bb0d781d35fd96ed32f46a568b527ae0b83d48e3))
* **terminal:** vendor xterm via build-time crate and surface debug image ([ff9e611](https://github.com/krezh/roder/commit/ff9e6113a8f583a2fad4e01fa513b16575590785))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#119](https://github.com/krezh/roder/issues/119)) ([aaab75e](https://github.com/krezh/roder/commit/aaab75e044db9a902d6f6cf670866fd9cb9e7f03))
* **cargo:** lock file maintenance cargo.lock ([#122](https://github.com/krezh/roder/issues/122)) ([6ee3c7a](https://github.com/krezh/roder/commit/6ee3c7a9c2e0e12f412c7db6ac971eba207757ca))
* **config:** migrate config .renovaterc.json5 ([#127](https://github.com/krezh/roder/issues/127)) ([1f37fc4](https://github.com/krezh/roder/commit/1f37fc4da44ab35db8ced19d97d667b609274c01))

## [0.1.36](https://github.com/krezh/roder/compare/0.1.35...0.1.36) (2026-07-23)


### Features

* **cargo:** update rust crate base64 (0.22.1 ➔ 0.23.0) ([#116](https://github.com/krezh/roder/issues/116)) ([a95e7f3](https://github.com/krezh/roder/commit/a95e7f3398311c8cd79113abf8f165cce0fa9b92))
* **delete:** add propagation policy and force options ([436fbff](https://github.com/krezh/roder/commit/436fbff3ca08dd9dbd7b1210a3bff2a694068474))
* **kopiur:** trigger manual snapshots on a SnapshotPolicy ([52f3c03](https://github.com/krezh/roder/commit/52f3c0326e9448b10f36fef21245da67d5fef0bb))

## [0.1.35](https://github.com/krezh/roder/compare/0.1.34...0.1.35) (2026-07-23)


### Features

* add manual Alertmanager refresh ([7ed93ea](https://github.com/krezh/roder/commit/7ed93eabae1217f4a714351929d993ec4c803ac5))


### Bug Fixes

* address code review findings ([b636036](https://github.com/krezh/roder/commit/b6360363b1129021e4fc10e6a46370d191d3f427))
* **cargo:** update rust crate leptos_router (0.8.14 ➔ 0.8.15) ([#111](https://github.com/krezh/roder/issues/111)) ([4abae58](https://github.com/krezh/roder/commit/4abae587c219723f61a6615df730071f12ddb6b7))
* **cargo:** update rust crate tokio-stream (0.1.18 ➔ 0.1.19) ([#114](https://github.com/krezh/roder/issues/114)) ([5a46fa1](https://github.com/krezh/roder/commit/5a46fa1eb1f522d54a1eff156779dacf4c650f2e))
* **container:** update image ghcr.io/rust-lang/rust (9a2cd30 ➔ 1bcff4b) ([#113](https://github.com/krezh/roder/issues/113)) ([1b57829](https://github.com/krezh/roder/commit/1b5782946b123343b16418223611524fc2b2b454))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#112](https://github.com/krezh/roder/issues/112)) ([e652063](https://github.com/krezh/roder/commit/e652063b3195ed005b1ce0245351210e55e93e7d))
* **cargo:** lock file maintenance cargo.lock ([#115](https://github.com/krezh/roder/issues/115)) ([95c6399](https://github.com/krezh/roder/commit/95c63992df9613dbf3361fdbf9c6fbabe96d4bba))

## [0.1.34](https://github.com/krezh/roder/compare/0.1.33...0.1.34) (2026-07-21)


### Bug Fixes

* **drain:** allow graceful proxy termination ([0f69b0d](https://github.com/krezh/roder/commit/0f69b0d8cbb595e929ff7d40f97685e98b9069ab))
* **drain:** wait indefinitely by default ([d6dabd2](https://github.com/krezh/roder/commit/d6dabd2354ea28458f919de1d5ac9092cdea4e7c))

## [0.1.33](https://github.com/krezh/roder/compare/0.1.32...0.1.33) (2026-07-21)


### Bug Fixes

* **talos:** use one-to-one proxying for COSI requests ([24bf2c8](https://github.com/krezh/roder/commit/24bf2c8e29e4db427601c2cadf47c2d60ecb2a04))

## [0.1.32](https://github.com/krezh/roder/compare/0.1.31...0.1.32) (2026-07-21)


### Performance Improvements

* speed up release builds ([9c40f5f](https://github.com/krezh/roder/commit/9c40f5fd8360f607cbedb72a82e136aa25d1b985))

## [0.1.31](https://github.com/krezh/roder/compare/0.1.30...0.1.31) (2026-07-21)


### Bug Fixes

* handle multi-document Talos configs and node targeting ([d9d06a6](https://github.com/krezh/roder/commit/d9d06a6a14160644030f896fdfc5a9f0a5d19e31))

## [0.1.30](https://github.com/krezh/roder/compare/0.1.29...0.1.30) (2026-07-21)


### Features

* support safe multi-replica node operations ([f627d39](https://github.com/krezh/roder/commit/f627d390c590c647366f5dcccb34184b1194e8b2))


### Bug Fixes

* **cargo:** update rust crate tokio (1.53.0 ➔ 1.53.1) ([#103](https://github.com/krezh/roder/issues/103)) ([3a62962](https://github.com/krezh/roder/commit/3a6296211d785e1b0112b58ac49836a83a4f3c58))


### Miscellaneous Chores

* **cargo:** lock file maintenance cargo.lock ([#105](https://github.com/krezh/roder/issues/105)) ([c5e6973](https://github.com/krezh/roder/commit/c5e69730106e15ac8e00afced62420fd679eaa6b))

## [0.1.29](https://github.com/krezh/roder/compare/0.1.28...0.1.29) (2026-07-20)


### Features

* **helm:** SA RBAC for shared enrichment, alertmanager URL, hardened security ([b41bf6d](https://github.com/krezh/roder/commit/b41bf6df32409f818db6f039d8a67a7ed9f1f6d4))
* multi-user cluster access via token passthrough ([522feec](https://github.com/krezh/roder/commit/522feecf41445efa22fb959de0c179b18fde85d7))


### Bug Fixes

* **cargo:** update rust crate serde_json (1.0.150 ➔ 1.0.151) ([#100](https://github.com/krezh/roder/issues/100)) ([6b9b453](https://github.com/krezh/roder/commit/6b9b4535387be6996c49933560f109dfcb65baab))
* **cargo:** update rust crate time (0.3.53 ➔ 0.3.54) ([#101](https://github.com/krezh/roder/issues/101)) ([14c3836](https://github.com/krezh/roder/commit/14c38368bb6518b3449470df689b91902b8a501b))
* **style:** use accent color for user identifier ([0f70f1f](https://github.com/krezh/roder/commit/0f70f1f2180df2d865a14734de897aa1d0566609))


### Continuous Integration

* **renovate:** enable automerge ([5e9ed0f](https://github.com/krezh/roder/commit/5e9ed0f97728705c060c1834550bdf909f5de5b1))
* run independent test steps in parallel ([dffb639](https://github.com/krezh/roder/commit/dffb6393fff85768f0f12696fe0be0fff50ddad6))

## [0.1.28](https://github.com/krezh/roder/compare/0.1.27...0.1.28) (2026-07-19)


### Bug Fixes

* **cargo:** update rust crate futures (0.3.32 ➔ 0.3.33) ([#96](https://github.com/krezh/roder/issues/96)) ([2bf08af](https://github.com/krezh/roder/commit/2bf08af744dd7fd3e3c32fc68eaeaedd3c15a0ab))
* **cargo:** update rust crate serde (1.0.228 ➔ 1.0.229) ([#99](https://github.com/krezh/roder/issues/99)) ([2ab7868](https://github.com/krezh/roder/commit/2ab7868d2cfe5924ac49154c406d4cf717425908))
* **cargo:** update rust crate thiserror (2.0.18 ➔ 2.0.19) ([#98](https://github.com/krezh/roder/issues/98)) ([46ecdf0](https://github.com/krezh/roder/commit/46ecdf0dee0bb74222c054b7639c8310243289c9))
* **chart:** pin image digest ([06cf0d6](https://github.com/krezh/roder/commit/06cf0d6eb61a014f1da9b3f847be442c1e5367e1))

## [0.1.27](https://github.com/krezh/roder/compare/0.1.26...0.1.27) (2026-07-18)


### Features

* **cargo:** update rust crate tokio (1.52.4 ➔ 1.53.0) ([#93](https://github.com/krezh/roder/issues/93)) ([dfcf935](https://github.com/krezh/roder/commit/dfcf935124e151463af0346da0d6f90dd7dd490c))
* **drain:** improve node drain ([#95](https://github.com/krezh/roder/issues/95)) ([3378c99](https://github.com/krezh/roder/commit/3378c99e2d8cc6ece275b3a679d433ca2facc73f))


### Bug Fixes

* **container:** update image ghcr.io/rust-lang/rust (1.97.0 ➔ 1.97.1) ([#92](https://github.com/krezh/roder/issues/92)) ([3b8bc1a](https://github.com/krezh/roder/commit/3b8bc1a2604469b63e449cb43299b89e81d0d94f))

## [0.1.26](https://github.com/krezh/roder/compare/0.1.25...0.1.26) (2026-07-16)


### Miscellaneous Chores

* **main:** release 0.1.25 ([#89](https://github.com/krezh/roder/issues/89)) ([7558d8a](https://github.com/krezh/roder/commit/7558d8a7b4117668d7c78bcef23b4c14a96fe5b0))

## [0.1.25](https://github.com/krezh/roder/compare/0.1.24...0.1.25) (2026-07-16)


### Features

* **cargo:** update rust crate sha2 (0.10.9 ➔ 0.11.0) ([#82](https://github.com/krezh/roder/issues/82)) ([160ffdd](https://github.com/krezh/roder/commit/160ffdd125391d1419d191de7e1b07c3a7007f30))
* **gui:** morph the user button into its dropdown as one continuous box ([7c81464](https://github.com/krezh/roder/commit/7c81464f07a55b5c87e64f2cb9232896f5a538d3))
* **gui:** move sign out into a user menu alongside Access ([6ca2926](https://github.com/krezh/roder/commit/6ca2926333d353f51319c82401ccb7bc0fcec0e6))


### Bug Fixes

* **cargo:** update rust crate tokio (1.52.3 ➔ 1.52.4) ([#88](https://github.com/krezh/roder/issues/88)) ([2cf43e7](https://github.com/krezh/roder/commit/2cf43e704713db7dfb78d307a99a117ac54c56b0))
* **container:** update image ghcr.io/rust-lang/rust (8e117ca ➔ b92b8c8) ([#86](https://github.com/krezh/roder/issues/86)) ([2723c9d](https://github.com/krezh/roder/commit/2723c9db3adaefe31696f128f8a8bf13409f07c1))
* **k8s:** allow forcing node drain past unmanaged/emptyDir pods ([1720fb1](https://github.com/krezh/roder/commit/1720fb1c4f962a156d5c4043b8a0a6e9f4e75e40))


### Code Refactoring

* **gui:** deduplicate the hover-tooltip reveal pattern ([9a1e8ee](https://github.com/krezh/roder/commit/9a1e8eeaf3e05e83240689c15746ffffcd66185b))
* **gui:** support multi-button confirm dialogs ([f42abf2](https://github.com/krezh/roder/commit/f42abf27d3903f386c32224f8a187e7865932346))
* split the four largest files into one-file-per-concern modules ([0a5bdec](https://github.com/krezh/roder/commit/0a5bdec5f7632ea03523dc959f8e40b4f9c27e91))

## [0.1.24](https://github.com/krezh/roder/compare/0.1.23...0.1.24) (2026-07-15)


### Features

* **app:** auto-reload the client when the server redeploys a newer build ([de4c84d](https://github.com/krezh/roder/commit/de4c84d603340930ae87cb050335fde6026e60ff))


### Bug Fixes

* **app:** satisfy newer clippy lints in the metrics chart animator ([9130aa7](https://github.com/krezh/roder/commit/9130aa71eb6d606a86c84898f3870ebf84fb8304))
* **helm:** drop default os:reader for talos.roles ([99a9a6b](https://github.com/krezh/roder/commit/99a9a6b5ff9e2137967d357b7e4ecb6978b911c9))
* **helm:** make the Talos ServiceAccount role fully configurable ([e49a995](https://github.com/krezh/roder/commit/e49a99507202139da472e9c2e6803f78902d337a))

## [0.1.23](https://github.com/krezh/roder/compare/0.1.22...0.1.23) (2026-07-15)


### Features

* **container:** update image ghcr.io/rust-lang/rust (1.96.1 ➔ 1.97.0) ([#79](https://github.com/krezh/roder/issues/79)) ([3101172](https://github.com/krezh/roder/commit/31011720b9dea2de8f25bd9a7db6205723c40117))
* **details:** improve graph visuals ([cab6853](https://github.com/krezh/roder/commit/cab6853707fb10698083915783bcd2faf3160120))
* **details:** Split the details menu metrics into separate stacked CPU and Memory graphs ([c511ad5](https://github.com/krezh/roder/commit/c511ad5a3aac05b6d390ec2206f4eba3aee72733))
* **gui:** Animate failure badges with spring transitions ([d4631f8](https://github.com/krezh/roder/commit/d4631f8b0757035dce1647022a989ae8e4ff6a8e))
* **gui:** Make the brand reflect live cluster connectivity ([3522479](https://github.com/krezh/roder/commit/352247993456d15f51bfe0537fc744d549427260))
* **mobile:** hold to multi-select, bigger touch/text scale, match new panel style ([5a7bdc4](https://github.com/krezh/roder/commit/5a7bdc41833a52adcc5ec518a124d8fd518c71df))
* **nodes:** add cordon/uncordon actions ([75cb2e1](https://github.com/krezh/roder/commit/75cb2e159f2c98eeb2cd548737af47f4129a8f97))
* **nodes:** add drain action ([cf7fab4](https://github.com/krezh/roder/commit/cf7fab4010e5a5cb5f2986a2c64ce3261052a78b))
* **nodes:** add node shell via privileged debug pod ([e4cfbea](https://github.com/krezh/roder/commit/e4cfbeac5dc8d8a502087011568fff792c9eaba0))
* **resources:** add RBAC access review ([6c50a70](https://github.com/krezh/roder/commit/6c50a70bbaa8441cde204ebbc646d88dd65e2cf3))
* **style:** improve sidebar ([2771a37](https://github.com/krezh/roder/commit/2771a37c92ed89634938bcdbf15f5d9312835be5))
* **style:** improve style of status boxes ([d4e35c0](https://github.com/krezh/roder/commit/d4e35c09c111772323793e151351255840130afb))
* **style:** improve topusage display with visual progress bars and accessibility enhancements ([2314e9c](https://github.com/krezh/roder/commit/2314e9c658dafbec3e8e04fc5b12893d3368133f))
* **talos:** add node diagnostics and actions ([8f0be82](https://github.com/krezh/roder/commit/8f0be82c8cbd3e2e717715fcd732d8129d3906af))
* **talos:** complete integrations ([6e88e8e](https://github.com/krezh/roder/commit/6e88e8ef48586cfa042843fb9ed6f61ad9cf4b07))
* **talos:** harden integration ([579b267](https://github.com/krezh/roder/commit/579b2672b3621fef8878936bbcd877b3387fb998))
* **talos:** integrate Talos machine API for node status ([c8d28b6](https://github.com/krezh/roder/commit/c8d28b641c3c87ebf61fccb4dbf706de399a6143))
* **ui:** animate details panel closing ([6d38099](https://github.com/krezh/roder/commit/6d3809971c54326c8b0f2d09d377074c7e41fb9f))
* **workloads:** add evict pod action ([a454453](https://github.com/krezh/roder/commit/a4544532683986948a480a056e4754223950a945))


### Bug Fixes

* **cargo:** update rust crate rustls (0.23.41 ➔ 0.23.42) ([#81](https://github.com/krezh/roder/issues/81)) ([6fd740c](https://github.com/krezh/roder/commit/6fd740c40ffdf442888ec0cf0ef4c56e509aea79))
* **clippy:** clippy error ([c3dbefb](https://github.com/krezh/roder/commit/c3dbefbe9ac4f62f2cedf5d05f4415364337d23a))
* **container:** update image gcr.io/distroless/cc-debian13 (a017e74 ➔ bc0f6c3) ([#77](https://github.com/krezh/roder/issues/77)) ([5a02786](https://github.com/krezh/roder/commit/5a027863d61ec4ff9d83297e7bf688e34c553dcd))
* **container:** update image gcr.io/distroless/cc-debian13 (bc0f6c3 ➔ ed7c407) ([#83](https://github.com/krezh/roder/issues/83)) ([4cd56d5](https://github.com/krezh/roder/commit/4cd56d5c5f4948e7e8209e55e97e0e7a9755fa11))
* **container:** update image ghcr.io/rust-lang/rust (44637ff ➔ 8e117ca) ([#84](https://github.com/krezh/roder/issues/84)) ([80f26bf](https://github.com/krezh/roder/commit/80f26bfa467bec4f896f9899eef38fa81c089f24))
* **gui:** Align pod and Flux failure badge labels ([cd400de](https://github.com/krezh/roder/commit/cd400dea4cc5e52bc7c1922bc7653bf28e853012))
* **gui:** Auto-refresh namespace list so it updates without a browser reload ([2da21c0](https://github.com/krezh/roder/commit/2da21c0d4a325477e82a8b8e8fea9b218ea9d914))
* **gui:** Clarify Flux failure counts and navigation ([6ea7039](https://github.com/krezh/roder/commit/6ea7039613ecd7de45aa8b1427b817452a6e4143))
* **gui:** Improve logs sizing and Flux failure navigation ([dc357f5](https://github.com/krezh/roder/commit/dc357f5beed1995278fa404ce89f8032a7e18155))
* **gui:** Keep context menus within the viewport ([5eebcb2](https://github.com/krezh/roder/commit/5eebcb2b048e811fc77cbc59d0a2fcf9a49a0f00))
* **gui:** Keep temporary health badges after permanent controls ([a5b294e](https://github.com/krezh/roder/commit/a5b294ede6ea6f5f98a6f2fb415fe45f175cc5e5))
* **k8s:** Back off watcher retries during cluster outages ([700cfb0](https://github.com/krezh/roder/commit/700cfb0c3cfbdf22b71eafa717ac7ea469ba9b87))
* **k8s:** Sweep terminally failed pods ([a0e790b](https://github.com/krezh/roder/commit/a0e790b9f372c224beb0ef5a04028c2f7f0077bb))
* **logs:** Fix log follow not scrolling to newest line ([e80adb6](https://github.com/krezh/roder/commit/e80adb6d88772965e43aca1943074bf9d5334d5c))
* **table:** fix numeric trend arrows ([a2525ce](https://github.com/krezh/roder/commit/a2525cec0368e89a2b43c7401eeb7cccda272c0b))


### Styles

* **details:** improve content hierarchy ([cffa846](https://github.com/krezh/roder/commit/cffa84666374707d354cb9508fd636345fcfb557))
* elevate/blur/box panels, unify sidebar+panel color ([c82c1a5](https://github.com/krezh/roder/commit/c82c1a51c253c336b7706acf687f5529872fd1df))
* remove vertical cell borders ([67de60a](https://github.com/krezh/roder/commit/67de60a05900dc44fb38750d73c065faf5dd1108))


### Code Refactoring

* reorganize topbar and enhance visual hierarchy ([bf56d07](https://github.com/krezh/roder/commit/bf56d07579f24afd91729d1f1fb0cd96f261a61a))

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
