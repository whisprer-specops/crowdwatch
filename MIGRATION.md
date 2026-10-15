# Migration

## From 0.1.0 (Python) to 0.2.0 (Rust)

0.2.0 is the same observatory written in Rust. These are unchanged:

- the commands and options
- configuration files, the three presets and the defaults
- the canonical and mapped import formats
- the database
- the JSON and HTML reports, and the backtest results
- the SkeleTrace export, including its identifiers
- the environment variables for keys and proxies
- the exit codes

The two versions read and write the same SQLite database, so 0.2.0 opens a 0.1.0 database as it is, and 0.1.0 can open one that 0.2.0 wrote. Back up a valuable database first, as before any upgrade.

Given the same data and the same `--at` time, the reports match byte for byte, apart from the version number. The demonstration at a fixed `--at` makes the same 396 observations, files and database rows.

To switch, build and install 0.2.0 as the README describes. Then type `crowdwatch` where you typed `py -3.14 -m crowdwatch`. There is no `run_demo.py` in 0.2.0. `crowdwatch demo --out .\my-demo`, then `Start-Process .\my-demo\report.html`, does the same job. `py .\tools\verify_skeletrace.py` becomes `crowdwatch verify-skeletrace`.

The Python release stays at the top of the repository (`crowdwatch/`, `tests/`, `tools/`, `run_demo.py` and `pyproject.toml`) for reference. Nothing in 0.2.0 runs it.

### Where the Python code went

| 0.1.0, in `crowdwatch/` | 0.2.0, in `v0.2.0/src/` |
| --- | --- |
| `cli.py`, `__main__.py`, `__init__.py` | `cli.rs`, `main.rs` |
| `config.py` | `config.rs` |
| `model.py` | `model.rs` |
| `store.py` | `store.rs` |
| `sources.py` | `sources.rs` |
| `http.py` | `http.rs` |
| `analysis.py` | `analysis.rs` |
| `report.py`, `assets/style.css` | `report.rs`, `v0.2.0/assets/style.css` |
| `export.py` | `export.rs` |
| `demo.py` | `demo.rs` |
| `tools/verify_skeletrace.py` | `cli.rs`, the `verify-skeletrace` command |
| The Python rules CrowdWatch relied on: JSON, number printing, string tests, `datetime`, `urllib.parse`, `csv`, ElementTree with its text encodings, `pathlib`, `shutil.which` and its exception names | `json.rs`, `pyfmt.rs`, `pytime.rs`, `urlparse.rs`, `csvread.rs`, `xml.rs`, `xml_encodings.rs`, `unicode_tables.rs`, `config.rs`, `cli.rs` and `error.rs` |
| `tests/test_contracts.py` (39 tests) | `v0.2.0/tests/contracts.rs` (the same 39) |
| `tests/test_sources.py` (12 tests) | `v0.2.0/tests/sources.rs` (the same 12) |
| Nothing | `v0.2.0/tests/cli_contracts.rs` (the program end to end) and `v0.2.0/tests/notice_contracts.rs` (copyright lines) |

### What is deliberately different

The version and the command line:

- **Version number.** Reports and `module.json` say `"version": "0.2.0"`, the report page's header says `SKELETRACE / CROWDWATCH 0.2`, and requests identify as `Skeletrace-CrowdWatch/0.2.0`.
- **`--version`** prints `0.2.0` on its first line, for scripts. The copyright, licence and no-warranty lines follow it.
- **Help.** The main help has a copyright line under its title and lists `verify-skeletrace`.
- **`verify-skeletrace`** replaces `tools/verify_skeletrace.py`. It runs the same checks and gives the same messages and exit codes. Three failures are worded in plain English rather than as Python's parser text. These are a profile or sample file that is not UTF-8 text, a profile that is not JSON, and host output that is not JSON.
- **`doctor`** reports `"platform"`, such as `"windows-x86_64"`, where 0.1.0 reported `"python"` and its version.
- **Plain help.** Help and usage errors are plain text. Python 3.14 coloured them when printing to a terminal.
- **No tracebacks.** Where 0.1.0 stopped with a Python traceback, 0.2.0 prints one line starting `crowdwatch:` and still exits with code 1. An example is an `--at` time so close to the year 9999 that the baseline window runs past the calendar.

Collecting and reading data:

- **A malformed reply fails only its own source.** 0.1.0 turned most reading errors into a per-source error, "Source I/O or schema error (KIND); no observations produced". Four kinds escaped that net and stopped the whole `collect` or `watch` with a traceback. These were `AttributeError` and `IndexError` (for example a list where a JSON reply should have an object), `LookupError` (an RSS feed naming a text encoding Python does not know) and `csv.Error` (a malformed CSV file). 0.2.0 records them as that source's error, in the same words, and carries on with the other sources.
- **Lone surrogates in JSON.** A `\ud800`-style escape with no partner is read as U+FFFD, the replacement character, and the record is kept. 0.1.0 read it but could not store it. `collect` reported the source as failed (`UnicodeEncodeError`), and `import` stopped with a traceback.
- **Nesting limit.** JSON nested more than 1,000 levels deep is refused as invalid JSON, on every system. 0.1.0's limit came from Python's own stack, so it varied with the Python version and the system, and usually lay several thousand levels deeper. Real feeds and imports come nowhere near either limit.
- **XML names.** RSS and Atom element names follow the character rules of XML 1.0 Fifth Edition. Python's expat parser uses older, narrower tables, so 0.1.0 refused a feed whose tag names used some newer letters.
- **Text encodings in feeds** are the ones Python 3.14 knows on any system. On Windows, Python could also borrow encodings from Windows itself, such as `mbcs` or rarer code pages like `cp28595`. A feed declaring one of those now fails as that source's error (`LookupError`). Feeds in UTF-8, UTF-16 and the common single-byte encodings are unaffected.
- **Times are read as Python 3.14 reads them**, the version 0.1.0's README used. Under Python 3.13 or older, 0.1.0 rejected `24:00`, which means midnight at the start of the next day. It also accepted some forms 3.14 refuses: a decimal mark on the hour or minute, as in `12.5`, and a colon before the microseconds, as in `12:00:00:5`. In the same way, Python's character tests, such as which characters count as spaces or digits, use Python 3.14's Unicode 16.0 tables, and option errors list choices without quotes.

The network:

- **Proxies.** Like 0.1.0, 0.2.0 reads `HTTPS_PROXY` (or `https_proxy`) and `NO_PROXY` (or `no_proxy`). It adds SOCKS proxies, including `socks5h://` for Tor. 0.1.0 spoke HTTP to a SOCKS port, which fails. An address it cannot use makes each request fail with "HTTPS_PROXY holds an unsupported proxy address" rather than go direct. With no variable set, 0.1.0 also used the Windows proxy settings; 0.2.0 does not read them.
- **Public addresses only, also past a proxy setting.** With a proxy set, 0.1.0 skipped its check that a feed's host resolves to public addresses, even for hosts in `NO_PROXY`, which it reached directly. 0.2.0 checks every direct connection as it is made, so such a host on a private address is refused.
- **Certificates.** HTTPS certificates are checked against the Mozilla root list built into the program, not the Windows certificate store. A network that inspects HTTPS with its own root certificate is refused.
- **International domain names** follow UTS #46, as browsers do: a feed on `straße.de` is fetched from `xn--strae-oqa.de`. 0.1.0 used Python's older IDNA 2003 rules, which turned that name into `strasse.de`, a different domain. Only a few characters, such as `ß` and `ς`, are treated differently.

The console:

- **Text encoding.** Output is always UTF-8. Lines end with CR LF on Windows, as Python's did. Under Python 3.14 on Windows, output redirected to a file used the system code page, and characters outside it stopped the program.
- **Ctrl+C** stops the program at once. 0.1.0 printed "CrowdWatch stopped." and exited with code 130. On Windows the exit code is now the system's own for Ctrl+C, which PowerShell shows as `-1073741510`. Linux and macOS shells still show 130. Nothing half-written is kept: a database change is either whole or rolled back, and a file is replaced only once fully written. A file save that is cut short can leave a stray `.crowdwatch-...` temporary file, which is safe to delete. As in 0.1.0, a source whose collection was interrupted is retried once its 10-minute claim runs out.

### How the port was checked

Each part was compared with the 0.1.0 code running on Python 3.13, with Python 3.14's rules applied where 3.13 differs. The comparisons covered:

- about 41,000 generated cases across JSON reading, time parsing, URL cleaning, observation checks, canonical imports, geohashes, distances and number formatting. About 8,200 of them were made-up provider replies, spread across all nine collector kinds.
- 36 random deployments, each run through 13 commands, comparing every printed output, file and database row
- 94 command lines covering help, usage errors, failures and exit codes

Apart from the items listed above, no differences were found. Running both versions' demonstrations at the same `--at` time gives identical files and database rows, apart from the version number.

SkeleTrace 1.0.0 accepted the 0.2.0 export through `crowdwatch verify-skeletrace`: it validated the profile and read all 15 sample records.

The Windows-only rules were tested on Linux with those rules switched on. They cover path spelling, program lookup and the wording of file errors. Windows itself was not run.

## From the supplied archive to 0.1.0

The original `osint-behavioural-sink` archive is preserved unmodified as `original-input.tar.gz`. `docs/ARCHIVE_REVIEW.md` records its full inventory, the audit of its mathematics and code, and what was kept. In short, its data-fusion idea became CrowdWatch, and its "behavioural sink" predictor was not carried over. The archive has no outcome labels, calibrated dynamics or trained model, and its sketches contain substantive errors. The original files are not imported or executed by either version.
