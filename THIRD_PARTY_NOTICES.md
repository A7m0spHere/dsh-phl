# 第三方声明 / THIRD_PARTY_NOTICES

PHL 依赖下列第三方组件。此清单由 `package.json` 与 `src-tauri/Cargo.lock` （license 字段读取自本地 npm/cargo 缓存）生成；发布前请用 `cargo about` 或 `npm ls` 重新生成并复核。各组件的许可证以其上游声明的完整文本为准。

## 前端 npm 依赖

| 组件 | 版本 | 许可证 | 来源 |
|---|---|---|---|
| @tauri-apps/api | 2.11.1 | Apache-2.0 OR MIT | [https://github.com/tauri-apps/tauri#readme](https://github.com/tauri-apps/tauri#readme) |
| @tauri-apps/cli | 2.11.4 | Apache-2.0 OR MIT | [https://github.com/tauri-apps/tauri#readme](https://github.com/tauri-apps/tauri#readme) |
| @tauri-apps/plugin-dialog | 2.7.3 | MIT OR Apache-2.0 | [https://www.npmjs.com/package/@tauri-apps/plugin-dialog](https://www.npmjs.com/package/@tauri-apps/plugin-dialog) |
| @types/node | 26.4.0 | MIT | [https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/node](https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/node) |
| @types/react | 18.3.31 | MIT | [https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react](https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react) |
| @types/react-dom | 18.3.7 | MIT | [https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react-dom](https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/react-dom) |
| @vitejs/plugin-react | 4.7.0 | MIT | [https://github.com/vitejs/vite-plugin-react/tree/main/packages/plugin-react#readme](https://github.com/vitejs/vite-plugin-react/tree/main/packages/plugin-react#readme) |
| autoprefixer | 10.5.4 | MIT | [https://www.npmjs.com/package/autoprefixer](https://www.npmjs.com/package/autoprefixer) |
| clsx | 2.1.1 | MIT | [https://www.npmjs.com/package/clsx](https://www.npmjs.com/package/clsx) |
| lucide-react | 0.454.0 | ISC | [https://lucide.dev](https://lucide.dev) |
| motion | 12.43.0 | MIT | [https://www.npmjs.com/package/motion](https://www.npmjs.com/package/motion) |
| postcss | 8.5.26 | MIT | [https://postcss.org/](https://postcss.org/) |
| react | 18.3.1 | MIT | [https://reactjs.org/](https://reactjs.org/) |
| react-dom | 18.3.1 | MIT | [https://reactjs.org/](https://reactjs.org/) |
| tailwind-merge | 2.6.1 | MIT | [https://github.com/dcastil/tailwind-merge](https://github.com/dcastil/tailwind-merge) |
| tailwindcss | 3.4.19 | MIT | [https://tailwindcss.com](https://tailwindcss.com) |
| typescript | 5.9.3 | Apache-2.0 | [https://www.typescriptlang.org/](https://www.typescriptlang.org/) |
| vite | 6.4.3 | MIT | [https://vite.dev](https://vite.dev) |
| vitest | 3.2.7 | MIT | [https://github.com/vitest-dev/vitest#readme](https://github.com/vitest-dev/vitest#readme) |
| zustand | 5.0.15 | MIT | [https://github.com/pmndrs/zustand](https://github.com/pmndrs/zustand) |

## Rust crates（含传递依赖，来自 Cargo.lock）

| 组件 | 许可证 |
|---|---|
| adler2 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| aho-corasick 1.1.5 | Unlicense OR MIT |
| alloc-no-stdlib 2.0.4 | BSD-3-Clause |
| alloc-stdlib 0.2.4 | BSD-3-Clause |
| android_system_properties 0.1.6 | 未在本地缓存中找到（cargo about 可补全） |
| anyhow 1.0.104 | MIT OR Apache-2.0 |
| arbitrary 1.4.2 | 未在本地缓存中找到（cargo about 可补全） |
| async-compression 0.4.43 | MIT OR Apache-2.0 |
| atk 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| atk-sys 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| atomic-waker 1.1.2 | Apache-2.0 OR MIT |
| autocfg 1.5.1 | Apache-2.0 OR MIT |
| base64 0.21.7 | 未在本地缓存中找到（cargo about 可补全） |
| base64 0.22.1 | MIT OR Apache-2.0 |
| bit-set 0.8.0 | Apache-2.0 OR MIT |
| bit-vec 0.8.0 | Apache-2.0 OR MIT |
| bitflags 1.3.2 | MIT/Apache-2.0 |
| bitflags 2.13.1 | MIT OR Apache-2.0 |
| block-buffer 0.10.4 | MIT OR Apache-2.0 |
| block2 0.6.2 | 未在本地缓存中找到（cargo about 可补全） |
| brotli 8.0.4 | BSD-3-Clause AND MIT |
| brotli-decompressor 5.0.3 | BSD-3-Clause/MIT |
| bs58 0.5.1 | MIT/Apache-2.0 |
| bumpalo 3.20.3 | MIT OR Apache-2.0 |
| bytemuck 1.25.2 | 未在本地缓存中找到（cargo about 可补全） |
| byteorder 1.5.0 | Unlicense OR MIT |
| bytes 1.12.1 | MIT |
| cairo-rs 0.18.5 | 未在本地缓存中找到（cargo about 可补全） |
| cairo-sys-rs 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| camino 1.2.5 | MIT OR Apache-2.0 |
| cargo-platform 0.1.9 | MIT OR Apache-2.0 |
| cargo_metadata 0.19.2 | MIT |
| cargo_toml 0.22.3 | Apache-2.0 OR MIT |
| cc 1.4.4 | MIT OR Apache-2.0 |
| cesu8 1.1.0 | 未在本地缓存中找到（cargo about 可补全） |
| cfb 0.7.3 | MIT |
| cfg-expr 0.15.8 | 未在本地缓存中找到（cargo about 可补全） |
| cfg-if 1.0.4 | MIT OR Apache-2.0 |
| cfg_aliases 0.2.2 | MIT |
| chacha20 0.10.2 | MIT OR Apache-2.0 |
| chrono 0.4.45 | MIT OR Apache-2.0 |
| combine 4.6.8 | 未在本地缓存中找到（cargo about 可补全） |
| compression-codecs 0.4.38 | MIT OR Apache-2.0 |
| compression-core 0.4.32 | MIT OR Apache-2.0 |
| cookie 0.18.2 | MIT OR Apache-2.0 |
| core-foundation 0.10.1 | 未在本地缓存中找到（cargo about 可补全） |
| core-foundation-sys 0.8.7 | 未在本地缓存中找到（cargo about 可补全） |
| core-graphics 0.25.0 | 未在本地缓存中找到（cargo about 可补全） |
| core-graphics-types 0.2.0 | 未在本地缓存中找到（cargo about 可补全） |
| cpufeatures 0.2.17 | MIT OR Apache-2.0 |
| cpufeatures 0.3.1 | MIT OR Apache-2.0 |
| crc32fast 1.5.1 | MIT OR Apache-2.0 |
| crossbeam-channel 0.5.16 | MIT OR Apache-2.0 |
| crossbeam-utils 0.8.22 | MIT OR Apache-2.0 |
| crypto-common 0.1.7 | MIT OR Apache-2.0 |
| cssparser 0.36.0 | MPL-2.0 |
| cssparser-macros 0.6.1 | MPL-2.0 |
| ctor 0.8.0 | Apache-2.0 OR MIT |
| ctor-proc-macro 0.0.7 | Apache-2.0 OR MIT |
| darling 0.23.0 | MIT |
| darling_core 0.23.0 | MIT |
| darling_macro 0.23.0 | MIT |
| dbus 0.9.12 | 未在本地缓存中找到（cargo about 可补全） |
| defmt 1.1.1 | MIT OR Apache-2.0 |
| defmt-macros 1.1.1 | MIT OR Apache-2.0 |
| defmt-parser 1.0.0 | MIT OR Apache-2.0 |
| deranged 0.5.8 | MIT OR Apache-2.0 |
| derive_arbitrary 1.4.2 | 未在本地缓存中找到（cargo about 可补全） |
| derive_more 2.1.1 | MIT |
| derive_more-impl 2.1.1 | MIT |
| digest 0.10.7 | MIT OR Apache-2.0 |
| dirs 5.0.1 | MIT OR Apache-2.0 |
| dirs 6.0.0 | MIT OR Apache-2.0 |
| dirs-sys 0.4.1 | MIT OR Apache-2.0 |
| dirs-sys 0.5.0 | MIT OR Apache-2.0 |
| dispatch2 0.3.1 | 未在本地缓存中找到（cargo about 可补全） |
| displaydoc 0.2.7 | MIT OR Apache-2.0 |
| dlopen2 0.8.2 | 未在本地缓存中找到（cargo about 可补全） |
| dlopen2_derive 0.4.3 | 未在本地缓存中找到（cargo about 可补全） |
| dom_query 0.27.0 | MIT |
| dpi 0.1.2 | Apache-2.0 AND MIT |
| dtoa 1.0.11 | MIT OR Apache-2.0 |
| dtoa-short 0.3.5 | MPL-2.0 |
| dtor 0.3.0 | Apache-2.0 OR MIT |
| dtor-proc-macro 0.0.6 | Apache-2.0 OR MIT |
| dunce 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| dyn-clone 1.0.20 | MIT OR Apache-2.0 |
| embed-resource 3.0.11 | MIT |
| embed_plist 1.2.2 | 未在本地缓存中找到（cargo about 可补全） |
| equivalent 1.0.2 | Apache-2.0 OR MIT |
| erased-serde 0.4.10 | MIT OR Apache-2.0 |
| errno 0.3.14 | 未在本地缓存中找到（cargo about 可补全） |
| fastrand 2.5.0 | Apache-2.0 OR MIT |
| fdeflate 0.3.7 | MIT OR Apache-2.0 |
| field-offset 0.3.6 | 未在本地缓存中找到（cargo about 可补全） |
| filetime 0.2.29 | MIT/Apache-2.0 |
| find-msvc-tools 0.1.11 | MIT OR Apache-2.0 |
| flate2 1.1.10 | MIT OR Apache-2.0 |
| fnv 1.0.7 | Apache-2.0 / MIT |
| foldhash 0.2.0 | Zlib |
| foreign-types 0.5.0 | 未在本地缓存中找到（cargo about 可补全） |
| foreign-types-macros 0.2.4 | 未在本地缓存中找到（cargo about 可补全） |
| foreign-types-shared 0.3.1 | 未在本地缓存中找到（cargo about 可补全） |
| form_urlencoded 1.2.2 | MIT OR Apache-2.0 |
| fs2 0.4.3 | MIT/Apache-2.0 |
| futures-channel 0.3.34 | MIT OR Apache-2.0 |
| futures-core 0.3.34 | MIT OR Apache-2.0 |
| futures-executor 0.3.34 | 未在本地缓存中找到（cargo about 可补全） |
| futures-io 0.3.34 | MIT OR Apache-2.0 |
| futures-macro 0.3.34 | MIT OR Apache-2.0 |
| futures-sink 0.3.34 | MIT OR Apache-2.0 |
| futures-task 0.3.34 | MIT OR Apache-2.0 |
| futures-util 0.3.34 | MIT OR Apache-2.0 |
| gdk 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gdk-pixbuf 0.18.5 | 未在本地缓存中找到（cargo about 可补全） |
| gdk-pixbuf-sys 0.18.0 | 未在本地缓存中找到（cargo about 可补全） |
| gdk-sys 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gdkwayland-sys 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gdkx11 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gdkx11-sys 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| generic-array 0.14.7 | MIT |
| getrandom 0.2.17 | MIT OR Apache-2.0 |
| getrandom 0.3.4 | MIT OR Apache-2.0 |
| getrandom 0.4.3 | MIT OR Apache-2.0 |
| gio 0.18.4 | 未在本地缓存中找到（cargo about 可补全） |
| gio-sys 0.18.1 | 未在本地缓存中找到（cargo about 可补全） |
| glib 0.18.5 | 未在本地缓存中找到（cargo about 可补全） |
| glib-macros 0.18.5 | 未在本地缓存中找到（cargo about 可补全） |
| glib-sys 0.18.1 | 未在本地缓存中找到（cargo about 可补全） |
| glob 0.3.4 | MIT OR Apache-2.0 |
| gobject-sys 0.18.0 | 未在本地缓存中找到（cargo about 可补全） |
| gtk 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gtk-sys 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| gtk3-macros 0.18.2 | 未在本地缓存中找到（cargo about 可补全） |
| hashbrown 0.12.3 | MIT OR Apache-2.0 |
| hashbrown 0.17.1 | MIT OR Apache-2.0 |
| heck 0.4.1 | 未在本地缓存中找到（cargo about 可补全） |
| heck 0.5.0 | MIT OR Apache-2.0 |
| hex 0.4.3 | MIT OR Apache-2.0 |
| html5ever 0.38.0 | MIT OR Apache-2.0 |
| http 1.5.0 | MIT OR Apache-2.0 |
| http-body 1.1.0 | MIT |
| http-body-util 0.1.5 | MIT |
| httparse 1.10.1 | MIT OR Apache-2.0 |
| hyper 1.11.1 | MIT |
| hyper-rustls 0.27.9 | Apache-2.0 OR ISC OR MIT |
| hyper-util 0.1.20 | MIT |
| iana-time-zone 0.1.65 | 未在本地缓存中找到（cargo about 可补全） |
| iana-time-zone-haiku 0.1.2 | 未在本地缓存中找到（cargo about 可补全） |
| ico 0.5.0 | MIT |
| icu_collections 2.3.0 | Unicode-3.0 |
| icu_locale_core 2.3.0 | Unicode-3.0 |
| icu_normalizer 2.3.0 | Unicode-3.0 |
| icu_normalizer_data 2.3.0 | Unicode-3.0 |
| icu_properties 2.3.0 | Unicode-3.0 |
| icu_properties_data 2.3.0 | Unicode-3.0 |
| icu_provider 2.3.1 | Unicode-3.0 |
| ident_case 1.0.1 | MIT/Apache-2.0 |
| idna 1.1.0 | MIT OR Apache-2.0 |
| idna_adapter 1.2.2 | Apache-2.0 OR MIT |
| indexmap 1.9.3 | Apache-2.0 OR MIT |
| indexmap 2.14.1 | Apache-2.0 OR MIT |
| infer 0.19.0 | MIT |
| ipnet 2.12.1 | MIT OR Apache-2.0 |
| itoa 1.0.18 | MIT OR Apache-2.0 |
| javascriptcore-rs 1.1.2 | 未在本地缓存中找到（cargo about 可补全） |
| javascriptcore-rs-sys 1.1.1 | 未在本地缓存中找到（cargo about 可补全） |
| jiff 0.2.35 | Unlicense OR MIT |
| jiff-core 0.1.0 | Unlicense OR MIT |
| jiff-static 0.2.35 | 未在本地缓存中找到（cargo about 可补全） |
| jiff-tzdb 0.1.8 | Unlicense OR MIT |
| jiff-tzdb-platform 0.1.3 | Unlicense OR MIT |
| jni 0.21.1 | 未在本地缓存中找到（cargo about 可补全） |
| jni-sys 0.3.1 | 未在本地缓存中找到（cargo about 可补全） |
| jni-sys 0.4.1 | 未在本地缓存中找到（cargo about 可补全） |
| jni-sys-macros 0.4.1 | 未在本地缓存中找到（cargo about 可补全） |
| js-sys 0.3.104 | 未在本地缓存中找到（cargo about 可补全） |
| json-patch 3.0.1 | MIT/Apache-2.0 |
| jsonptr 0.6.3 | MIT OR Apache-2.0 |
| keyboard-types 0.7.0 | MIT OR Apache-2.0 |
| libappindicator 0.9.0 | 未在本地缓存中找到（cargo about 可补全） |
| libappindicator-sys 0.9.0 | 未在本地缓存中找到（cargo about 可补全） |
| libc 0.2.189 | MIT OR Apache-2.0 |
| libdbus-sys 0.2.7 | 未在本地缓存中找到（cargo about 可补全） |
| libloading 0.7.4 | 未在本地缓存中找到（cargo about 可补全） |
| libredox 0.1.21 | 未在本地缓存中找到（cargo about 可补全） |
| linux-raw-sys 0.12.1 | 未在本地缓存中找到（cargo about 可补全） |
| litemap 0.8.3 | Unicode-3.0 |
| lock_api 0.4.14 | MIT OR Apache-2.0 |
| log 0.4.34 | MIT OR Apache-2.0 |
| lru-slab 0.1.2 | MIT OR Apache-2.0 OR Zlib |
| markup5ever 0.38.0 | MIT OR Apache-2.0 |
| memchr 2.8.3 | Unlicense OR MIT |
| memoffset 0.9.1 | 未在本地缓存中找到（cargo about 可补全） |
| mime 0.3.17 | MIT OR Apache-2.0 |
| miniz_oxide 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| mio 1.2.2 | MIT |
| muda 0.19.3 | Apache-2.0 OR MIT |
| ndk 0.9.0 | 未在本地缓存中找到（cargo about 可补全） |
| ndk-sys 0.6.0+11769913 | 未在本地缓存中找到（cargo about 可补全） |
| new_debug_unreachable 1.0.6 | MIT |
| num-conv 0.2.2 | MIT OR Apache-2.0 |
| num-traits 0.2.19 | MIT OR Apache-2.0 |
| num_enum 0.7.6 | 未在本地缓存中找到（cargo about 可补全） |
| num_enum_derive 0.7.6 | 未在本地缓存中找到（cargo about 可补全） |
| objc2 0.6.4 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-app-kit 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-cloud-kit 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-data 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-foundation 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-graphics 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-image 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-location 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-core-text 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-encode 4.1.0 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-exception-helper 0.1.1 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-foundation 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-io-surface 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-quartz-core 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-ui-kit 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-user-notifications 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| objc2-web-kit 0.3.2 | 未在本地缓存中找到（cargo about 可补全） |
| once_cell 1.21.4 | MIT OR Apache-2.0 |
| option-ext 0.2.0 | MPL-2.0 |
| pango 0.18.3 | 未在本地缓存中找到（cargo about 可补全） |
| pango-sys 0.18.0 | 未在本地缓存中找到（cargo about 可补全） |
| parking_lot 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core 0.9.12 | MIT OR Apache-2.0 |
| percent-encoding 2.3.2 | MIT OR Apache-2.0 |
| phf 0.13.1 | MIT |
| phf_codegen 0.13.1 | MIT |
| phf_generator 0.13.1 | MIT |
| phf_macros 0.13.1 | MIT |
| phf_shared 0.13.1 | MIT |
| pin-project-lite 0.2.17 | Apache-2.0 OR MIT |
| pkg-config 0.3.34 | 未在本地缓存中找到（cargo about 可补全） |
| plist 1.10.0 | MIT |
| png 0.17.16 | MIT OR Apache-2.0 |
| png 0.18.1 | 未在本地缓存中找到（cargo about 可补全） |
| portable-atomic 1.15.0 | 未在本地缓存中找到（cargo about 可补全） |
| portable-atomic-util 0.2.7 | 未在本地缓存中找到（cargo about 可补全） |
| potential_utf 0.1.6 | Unicode-3.0 |
| powerfmt 0.2.0 | MIT OR Apache-2.0 |
| precomputed-hash 0.1.1 | MIT |
| proc-macro-crate 1.3.1 | 未在本地缓存中找到（cargo about 可补全） |
| proc-macro-crate 2.0.2 | 未在本地缓存中找到（cargo about 可补全） |
| proc-macro-crate 3.5.0 | 未在本地缓存中找到（cargo about 可补全） |
| proc-macro-error 1.0.4 | 未在本地缓存中找到（cargo about 可补全） |
| proc-macro-error-attr 1.0.4 | 未在本地缓存中找到（cargo about 可补全） |
| proc-macro2 1.0.107 | MIT OR Apache-2.0 |
| quick-xml 0.41.0 | MIT |
| quinn 0.11.11 | MIT OR Apache-2.0 |
| quinn-proto 0.11.17 | MIT OR Apache-2.0 |
| quinn-udp 0.5.15 | MIT OR Apache-2.0 |
| quote 1.0.47 | MIT OR Apache-2.0 |
| r-efi 5.3.0 | 未在本地缓存中找到（cargo about 可补全） |
| r-efi 6.0.0 | 未在本地缓存中找到（cargo about 可补全） |
| rand 0.10.2 | MIT OR Apache-2.0 |
| rand_core 0.10.1 | MIT OR Apache-2.0 |
| rand_pcg 0.10.2 | MIT OR Apache-2.0 |
| raw-window-handle 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| redox_syscall 0.5.18 | 未在本地缓存中找到（cargo about 可补全） |
| redox_users 0.4.6 | 未在本地缓存中找到（cargo about 可补全） |
| redox_users 0.5.2 | 未在本地缓存中找到（cargo about 可补全） |
| ref-cast 1.0.27 | MIT OR Apache-2.0 |
| ref-cast-impl 1.0.27 | MIT OR Apache-2.0 |
| regex 1.13.1 | MIT OR Apache-2.0 |
| regex-automata 0.4.18 | MIT OR Apache-2.0 |
| regex-syntax 0.8.11 | MIT OR Apache-2.0 |
| reqwest 0.12.28 | MIT OR Apache-2.0 |
| reqwest 0.13.4 | 未在本地缓存中找到（cargo about 可补全） |
| rfd 0.16.0 | MIT |
| ring 0.17.14 | Apache-2.0 AND ISC |
| rustc-hash 2.1.3 | Apache-2.0 OR MIT |
| rustc_version 0.4.1 | MIT OR Apache-2.0 |
| rustix 1.1.4 | 未在本地缓存中找到（cargo about 可补全） |
| rustls 0.23.43 | Apache-2.0 OR ISC OR MIT |
| rustls-pki-types 1.15.1 | MIT OR Apache-2.0 |
| rustls-webpki 0.103.15 | ISC |
| rustversion 1.0.23 | 未在本地缓存中找到（cargo about 可补全） |
| ryu 1.0.23 | Apache-2.0 OR BSL-1.0 |
| same-file 1.0.6 | Unlicense/MIT |
| schemars 0.8.22 | MIT |
| schemars 0.9.0 | MIT |
| schemars 1.2.2 | MIT |
| schemars_derive 0.8.22 | MIT |
| scopeguard 1.2.0 | MIT OR Apache-2.0 |
| selectors 0.36.1 | MPL-2.0 |
| semver 1.0.28 | MIT OR Apache-2.0 |
| serde 1.0.229 | MIT OR Apache-2.0 |
| serde-untagged 0.1.9 | MIT OR Apache-2.0 |
| serde_core 1.0.229 | MIT OR Apache-2.0 |
| serde_derive 1.0.229 | MIT OR Apache-2.0 |
| serde_derive_internals 0.29.1 | MIT OR Apache-2.0 |
| serde_json 1.0.151 | MIT OR Apache-2.0 |
| serde_repr 0.1.21 | MIT OR Apache-2.0 |
| serde_spanned 0.6.9 | 未在本地缓存中找到（cargo about 可补全） |
| serde_spanned 1.1.1 | MIT OR Apache-2.0 |
| serde_urlencoded 0.7.1 | MIT/Apache-2.0 |
| serde_with 3.22.0 | MIT OR Apache-2.0 |
| serde_with_macros 3.22.0 | MIT OR Apache-2.0 |
| serde_yaml 0.9.34+deprecated | MIT OR Apache-2.0 |
| serialize-to-javascript 0.1.2 | MIT OR Apache-2.0 |
| serialize-to-javascript-impl 0.1.2 | MIT OR Apache-2.0 |
| servo_arc 0.4.3 | MIT OR Apache-2.0 |
| sha2 0.10.9 | MIT OR Apache-2.0 |
| shlex 2.0.1 | MIT OR Apache-2.0 |
| signal-hook-registry 1.4.8 | 未在本地缓存中找到（cargo about 可补全） |
| simd-adler32 0.3.10 | MIT |
| siphasher 1.0.3 | MIT/Apache-2.0 |
| slab 0.4.12 | MIT |
| smallvec 1.15.2 | MIT OR Apache-2.0 |
| socket2 0.6.5 | MIT OR Apache-2.0 |
| softbuffer 0.4.8 | MIT OR Apache-2.0 |
| soup3 0.5.0 | 未在本地缓存中找到（cargo about 可补全） |
| soup3-sys 0.5.0 | 未在本地缓存中找到（cargo about 可补全） |
| stable_deref_trait 1.2.1 | MIT OR Apache-2.0 |
| string_cache 0.9.0 | MIT OR Apache-2.0 |
| string_cache_codegen 0.6.1 | MIT OR Apache-2.0 |
| strsim 0.11.1 | MIT |
| subtle 2.6.1 | BSD-3-Clause |
| swift-rs 1.0.8 | 未在本地缓存中找到（cargo about 可补全） |
| syn 1.0.109 | 未在本地缓存中找到（cargo about 可补全） |
| syn 2.0.119 | MIT OR Apache-2.0 |
| syn 3.0.4 | MIT OR Apache-2.0 |
| sync_wrapper 1.0.2 | Apache-2.0 |
| synstructure 0.13.2 | MIT |
| system-deps 6.2.2 | 未在本地缓存中找到（cargo about 可补全） |
| tao 0.35.3 | Apache-2.0 |
| tao-macros 0.1.4 | 未在本地缓存中找到（cargo about 可补全） |
| tar 0.4.46 | MIT OR Apache-2.0 |
| target-lexicon 0.12.16 | 未在本地缓存中找到（cargo about 可补全） |
| tauri 2.11.5 | Apache-2.0 OR MIT |
| tauri-build 2.6.3 | Apache-2.0 OR MIT |
| tauri-codegen 2.6.3 | Apache-2.0 OR MIT |
| tauri-macros 2.6.3 | Apache-2.0 OR MIT |
| tauri-plugin 2.6.3 | Apache-2.0 OR MIT |
| tauri-plugin-dialog 2.7.3 | Apache-2.0 OR MIT |
| tauri-plugin-fs 2.5.2 | Apache-2.0 OR MIT |
| tauri-runtime 2.11.3 | Apache-2.0 OR MIT |
| tauri-runtime-wry 2.11.4 | Apache-2.0 OR MIT |
| tauri-utils 2.9.3 | Apache-2.0 OR MIT |
| tauri-winres 0.3.6 | MIT |
| tendril 0.5.1 | MIT OR Apache-2.0 |
| thiserror 1.0.69 | MIT OR Apache-2.0 |
| thiserror 2.0.20 | MIT OR Apache-2.0 |
| thiserror-impl 1.0.69 | MIT OR Apache-2.0 |
| thiserror-impl 2.0.20 | MIT OR Apache-2.0 |
| time 0.3.55 | MIT OR Apache-2.0 |
| time-core 0.1.9 | MIT OR Apache-2.0 |
| time-macros 0.2.32 | MIT OR Apache-2.0 |
| tinystr 0.8.4 | Unicode-3.0 |
| tinyvec 1.12.0 | Zlib OR Apache-2.0 OR MIT |
| tinyvec_macros 0.1.1 | MIT OR Apache-2.0 OR Zlib |
| tokio 1.53.1 | MIT |
| tokio-macros 2.7.2 | MIT |
| tokio-rustls 0.26.4 | MIT OR Apache-2.0 |
| tokio-util 0.7.19 | MIT |
| toml 0.8.2 | 未在本地缓存中找到（cargo about 可补全） |
| toml 0.9.12+spec-1.1.0 | MIT OR Apache-2.0 |
| toml 1.1.4+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime 0.6.3 | 未在本地缓存中找到（cargo about 可补全） |
| toml_datetime 0.7.5+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_datetime 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_edit 0.19.15 | 未在本地缓存中找到（cargo about 可补全） |
| toml_edit 0.20.2 | 未在本地缓存中找到（cargo about 可补全） |
| toml_edit 0.25.13+spec-1.1.0 | 未在本地缓存中找到（cargo about 可补全） |
| toml_parser 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 |
| toml_writer 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| tower 0.5.3 | MIT |
| tower-http 0.6.11 | MIT |
| tower-layer 0.3.3 | MIT |
| tower-service 0.3.3 | MIT |
| tracing 0.1.44 | MIT |
| tracing-core 0.1.36 | MIT |
| tray-icon 0.24.2 | MIT OR Apache-2.0 |
| try-lock 0.2.5 | MIT |
| typeid 1.0.3 | MIT OR Apache-2.0 |
| typenum 1.20.1 | MIT OR Apache-2.0 |
| unic-char-property 0.9.0 | MIT/Apache-2.0 |
| unic-char-range 0.9.0 | MIT/Apache-2.0 |
| unic-common 0.9.0 | MIT/Apache-2.0 |
| unic-ucd-ident 0.9.0 | MIT/Apache-2.0 |
| unic-ucd-version 0.9.0 | MIT/Apache-2.0 |
| unicode-ident 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-segmentation 1.13.3 | MIT OR Apache-2.0 |
| unsafe-libyaml 0.2.11 | MIT |
| untrusted 0.9.0 | ISC |
| url 2.5.8 | MIT OR Apache-2.0 |
| urlpattern 0.3.0 | MIT |
| utf8_iter 1.0.4 | Apache-2.0 OR MIT |
| uuid 1.26.0 | Apache-2.0 OR MIT |
| version-compare 0.2.1 | 未在本地缓存中找到（cargo about 可补全） |
| version_check 0.9.5 | MIT/Apache-2.0 |
| vswhom 0.1.0 | MIT |
| vswhom-sys 0.1.3 | MIT |
| walkdir 2.5.0 | Unlicense/MIT |
| want 0.3.1 | MIT |
| wasi 0.11.1+wasi-snapshot-preview1 | 未在本地缓存中找到（cargo about 可补全） |
| wasip2 1.0.4+wasi-0.2.12 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-bindgen 0.2.127 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-bindgen-futures 0.4.77 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-bindgen-macro 0.2.127 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-bindgen-macro-support 0.2.127 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-bindgen-shared 0.2.127 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-streams 0.4.2 | 未在本地缓存中找到（cargo about 可补全） |
| wasm-streams 0.5.0 | 未在本地缓存中找到（cargo about 可补全） |
| web-sys 0.3.104 | 未在本地缓存中找到（cargo about 可补全） |
| web-time 1.1.0 | 未在本地缓存中找到（cargo about 可补全） |
| web_atoms 0.2.6 | MIT OR Apache-2.0 |
| webkit2gtk 2.0.2 | 未在本地缓存中找到（cargo about 可补全） |
| webkit2gtk-sys 2.0.2 | 未在本地缓存中找到（cargo about 可补全） |
| webpki-roots 1.0.9 | CDLA-Permissive-2.0 |
| webview2-com 0.38.2 | MIT |
| webview2-com-macros 0.8.1 | MIT |
| webview2-com-sys 0.38.2 | MIT |
| winapi 0.3.9 | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu 0.4.0 | 未在本地缓存中找到（cargo about 可补全） |
| winapi-util 0.1.11 | Unlicense OR MIT |
| winapi-x86_64-pc-windows-gnu 0.4.0 | 未在本地缓存中找到（cargo about 可补全） |
| window-vibrancy 0.6.0 | Apache-2.0 OR MIT |
| windows 0.61.3 | MIT OR Apache-2.0 |
| windows-collections 0.2.0 | MIT OR Apache-2.0 |
| windows-core 0.61.2 | MIT OR Apache-2.0 |
| windows-core 0.62.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows-future 0.2.1 | MIT OR Apache-2.0 |
| windows-implement 0.60.2 | MIT OR Apache-2.0 |
| windows-interface 0.59.3 | MIT OR Apache-2.0 |
| windows-link 0.1.3 | MIT OR Apache-2.0 |
| windows-link 0.2.1 | MIT OR Apache-2.0 |
| windows-numerics 0.2.0 | MIT OR Apache-2.0 |
| windows-result 0.3.4 | MIT OR Apache-2.0 |
| windows-result 0.4.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows-strings 0.4.2 | MIT OR Apache-2.0 |
| windows-strings 0.5.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows-sys 0.45.0 | 未在本地缓存中找到（cargo about 可补全） |
| windows-sys 0.48.0 | MIT OR Apache-2.0 |
| windows-sys 0.52.0 | 未在本地缓存中找到（cargo about 可补全） |
| windows-sys 0.59.0 | MIT OR Apache-2.0 |
| windows-sys 0.60.2 | MIT OR Apache-2.0 |
| windows-sys 0.61.2 | MIT OR Apache-2.0 |
| windows-targets 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows-targets 0.48.5 | MIT OR Apache-2.0 |
| windows-targets 0.52.6 | MIT OR Apache-2.0 |
| windows-targets 0.53.5 | MIT OR Apache-2.0 |
| windows-threading 0.1.0 | MIT OR Apache-2.0 |
| windows-version 0.1.7 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_gnullvm 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_gnullvm 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_gnullvm 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_msvc 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_msvc 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_msvc 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_aarch64_msvc 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnu 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnu 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnu 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnu 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnullvm 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_gnullvm 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_msvc 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_msvc 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_msvc 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_i686_msvc 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnu 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnu 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnu 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnu 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnullvm 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnullvm 0.48.5 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnullvm 0.52.6 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_gnullvm 0.53.1 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_msvc 0.42.2 | 未在本地缓存中找到（cargo about 可补全） |
| windows_x86_64_msvc 0.48.5 | MIT OR Apache-2.0 |
| windows_x86_64_msvc 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc 0.53.1 | MIT OR Apache-2.0 |
| winnow 0.5.40 | 未在本地缓存中找到（cargo about 可补全） |
| winnow 0.7.15 | MIT |
| winnow 1.0.4 | MIT |
| winreg 0.55.0 | MIT |
| wit-bindgen 0.57.1 | 未在本地缓存中找到（cargo about 可补全） |
| writeable 0.6.4 | Unicode-3.0 |
| wry 0.55.1 | Apache-2.0 OR MIT |
| x11 2.21.0 | 未在本地缓存中找到（cargo about 可补全） |
| x11-dl 2.21.0 | 未在本地缓存中找到（cargo about 可补全） |
| xattr 1.6.1 | 未在本地缓存中找到（cargo about 可补全） |
| yoke 0.8.3 | Unicode-3.0 |
| yoke-derive 0.8.2 | Unicode-3.0 |
| zerofrom 0.1.8 | Unicode-3.0 |
| zerofrom-derive 0.1.7 | Unicode-3.0 |
| zeroize 1.9.0 | Apache-2.0 OR MIT |
| zerotrie 0.2.5 | Unicode-3.0 |
| zerovec 0.11.8 | Unicode-3.0 |
| zerovec-derive 0.11.6 | Unicode-3.0 |
| zip 2.4.2 | MIT |
| zlib-rs 0.6.7 | Zlib |
| zmij 1.0.23 | MIT |
| zopfli 0.8.3 | Apache-2.0 |

## 品牌与素材

应用内的标识、图标与配色体系均在本仓库内定义；未使用第三方项目的 Logo 或品牌资产。
图标由 `scripts/make-icon.mjs` 从本仓库内的源图生成。

## 设计与数据源（非分发依赖）

以下不是随应用分发的代码依赖，而是本仓库某功能的设计参考或外部数据源，按 #15 记录归属：

- **模型能力自动补全**（`src-tauri/src/api_config/catalog.rs`）：产品思路参考
  [`dsh-model-info-fill`](https://github.com/11zld22/dsh-model-info-fill)（**MIT**）。
  本仓库为**独立重新实现**（TS/Rust，PHL 原生功能，非 DSH 插件）；匹配器有意与原实现
  不同（provider 约束 + 歧义不猜，见 #5），不复制其源码。
- **模型目录数据源**：运行时从 [`models.dev`](https://models.dev) 的
  `https://models.dev/api.json` 拉取（仅 enrichment 用途，缓存于 `<root>/cache/models-dev.json`，
  非硬依赖，失败即降级）。该站点数据的许可以其上游声明为准。

## 已知缺口

- Rust crate 的许可证读取自本机 cargo 缓存；个别未缓存条目标注为「未找到」，
  发布前用 `cargo install cargo-about && cargo about generate` 复核。
- npm 依赖只列直接依赖（含 dev）；传递依赖随上游分发，许可证同上游。
