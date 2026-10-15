# Ownership, licence and third-party material

CrowdWatch is Copyright (C) 2026 whispr.dev. It is free software under the GNU General Public License, version 3 only (SPDX: `GPL-3.0-only`); the full text is in `LICENSE`. It comes with no warranty, to the extent permitted by law.


SkeleTrace's source code is not included. 0.1.0 read its interface from `whisprer-specops/osint-skeletrace` at commit `befa8404fb6f494315eeaa3639f263498fb89e26`, under `previous-versions/st12fix`. 0.2.0's export was checked with SkeleTrace 1.0.0, built from `whisprer-specops/osint-skeletrace-standalone` at commit `29fbef24071ea64b2002091a5f403a31c6bcef75`. `docs/SKELETRACE.md` has the details.

Public data keep their providers' attribution and usage terms. Synthetic fixtures are clearly marked and do not describe real people or real incidents.

The 0.1.0 implementation and documentation were created with OpenAI ChatGPT/Codex assistance. The 0.2.0 Rust port and its documentation were created with Anthropic's Claude. No claim of empirical model validation follows from the software tests.

## Libraries in the 0.2.0 program

0.2.0 is built with these Rust libraries, at the exact versions in `v0.2.0/Cargo.lock`. All are compatible with GPL-3.0-only.

| Library | Use | Licence |
| --- | --- | --- |
| `rusqlite`, `libsqlite3-sys` | The database; they bundle SQLite itself, which is in the public domain | MIT |
| `ureq`, `socks` | HTTPS requests and SOCKS proxies | MIT or Apache-2.0 |
| `rustls`, `rustls-webpki`, `rustls-pki-types` | TLS and certificate checks | Apache-2.0, ISC or MIT; ISC; MIT or Apache-2.0 |
| `webpki-roots` | Mozilla's root certificates | CDLA-Permissive-2.0 |
| `ring` | Cryptography, and SHA-256 for keys and digests | Apache-2.0 and ISC |
| `uuid`, `sha1_smol` | SkeleTrace identifiers | Apache-2.0 or MIT; BSD-3-Clause |
| `icu_normalizer` and the ICU4X crates it uses | Unicode normalisation in URL checks | Unicode-3.0 |
| `idna` and the `url` support crates | International domain names | MIT or Apache-2.0 |

Smaller support crates use MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0 or Unlicense terms. `cargo tree --locked --format "{p} {l}"`, run in `v0.2.0/`, lists every one with its licence.

The character tables in `v0.2.0/src/unicode_tables.rs` are derived from the Unicode Character Database, copyright Unicode, Inc., under the Unicode License v3. The single-byte tables in `v0.2.0/src/xml_encodings.rs` are derived from Python's text codecs, under the Python Software Foundation License. The 0.1.0 Python code uses only Python's standard library.
