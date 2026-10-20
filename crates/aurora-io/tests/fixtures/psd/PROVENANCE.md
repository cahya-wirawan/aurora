# PSD/PSB reader fixtures — provenance

These files are copied unmodified from psd-tools' own test suite
(<https://github.com/psd-tools/psd-tools>, `tests/psd_files/`, pinned at
commit `ad89f315777866c832bf82e0377226cb13250c36` — the same pin
`corpora/psd/reference/fetch-samples.sh` fetches). psd-tools is
MIT-licensed; its licence is reproduced in `LICENSE` beside this file and
covers these fixtures (the repository's fixtures carry no separate
licence). `4x4_8bit_lab.psd`, `4x4_8bit_grayscale.psd` and
`4x4_16bit_grayscale.psd` (added 0.147.0) come from that suite's
`colormodes/` subdirectory, as do `4x4_8bit_rgb.psd`, `4x4_16bit_lab.psd`,
`4x4_1bit_bitmap.psd`, `4x4_8bit_index_color.psd`,
`4x4_16bit_multichannel.psd` and `4x4_8bit_duotone.psd` (added in task 148).

They are committed (unlike the gitignored corpus under `corpora/`) because
`aurora-io`'s unit tests (`src/psd/tests.rs`) read them with
`include_bytes!`, so the tests run on every checkout and in CI. Expected
values in those tests were read independently with psd-tools 1.17.4.

| File | sha256 |
|---|---|
| `0layers.psd` | `0711306e56002f193e33aefe4a1e9ce89ceafaea04da243593bc0c1e697c7d64` |
| `16bit5x5.psd` | `c78689ea7b576f23bbd8b6b4f4993a365266c3ffc9aaa02cc20bef4a36a9d7a0` |
| `1layer.psd` | `c2b457581d549f4bea2e5c34a04b68b2917f640bce0e958b6bca9c65be75174b` |
| `1layer.psb` | `85317ccdb7ec11a51f1983ad2055544a490839c01f8b313db8070c1b0f0b83f4` |
| `2layers.psd` | `406ddf7cbf5a1065b992b9c0d2a59be6290a371b90f23b9a04c0e6bbc94beb0c` |
| `32bit5x5.psd` | `66a4260e6bb8a60deff94031a66fa502c74870d75a723b4180aef02c35a97d0e` |
| `4x4_8bit_grayscale.psd` | `2811614db8536c363ffd4b9c97baf8965b58fda5c894ffeffe1067d1287edacf` |
| `4x4_8bit_lab.psd` | `48e52730a29cf715f9df9be12d6730af9e8125d27c5293910fedd7d621cf03e9` |
| `4x4_16bit_grayscale.psd` | `5ee15880f77c1a0e2566864d8770a26efe3aa261245e360267973ebdd542a49c` |
| `group.psd` | `1aaf572b69f0b0fa7c04b7b3a14c97e310a0c7bbdc06e733c9dd0d0cdcf7df1c` |
| `hidden-layer.psd` | `3aaadd20b5ff9d5e778239ee734c9c2bf57fc56bccc82c1b003c3719df46dba0` |
| `layer-name-emoji.psd` | `2ed49149deeffbccb89aebbf8c5a19dee12a864b17641c7bc03a89e957f3eaa8` |
| `4x4_8bit_rgb.psd` | `b656bd96b10350fc6911afaabbc2fa6a48cf5c1ae59c1a8e2f952539bd4f1930` |
| `4x4_16bit_lab.psd` | `35f1727b45b73eb6191a726937f336c8ef6515992bd4913c80e67f46dbc5f12b` |
| `4x4_1bit_bitmap.psd` | `56ec3967706373af835a319aa2ef6698351e07b8b495e5296ba4fe8848c2dd5f` |
| `4x4_8bit_index_color.psd` | `1da88234b4c1d33ef2fed6a3a496f68e3c28c0bce420400917736e9b72f194cc` |
| `4x4_16bit_multichannel.psd` | `36730fdab702fbbde8f8b96451d1099186a4443d02180119542ed50aab34c88d` |
| `4x4_8bit_duotone.psd` | `4169c367a7e4698f55955901849c38b8bd284dd61f8ddcbef8bafa70e54bb19b` |
