# Security notes

This covers CrowdWatch 0.2.0, the Rust release in `v0.2.0/`. Report vulnerabilities privately through the repository's security advisory feature: <https://github.com/whisprer-specops/crowdwatch/security/advisories/new>. Include the version, the affected command or collector, reproducible steps and the likely impact, using synthetic inputs and redacted credentials.

- CrowdWatch works with aggregate measurements. Its observation format has no place for personal identifiers; do not configure feeds that describe individuals.
- Configurations, reports, exports and the demo's feed files are written readable only by their owner on Linux and macOS. The database follows the system's default permissions. On Windows, access follows the folder's permissions.
- Keys are read from the environment variables a configuration names. They are not stored in the database or written to reports or exports. Source URLs are stored without their query strings. Error messages name the host at most; they never repeat a request URL, key or response body.
- Feeds use HTTPS on port 443 only. Redirects are refused, so a key is never forwarded to another host. Each network step may take 15 seconds, a reply may not exceed 5 MiB, and compressed replies are refused.
- A feed's host must resolve only to public internet addresses. When CrowdWatch connects directly, the check is repeated at connection time, so a host cannot switch to a private address in between. Host names such as `localhost` and names ending in `.local` or `.internal` are refused.
- RSS and Atom feeds may not contain DTDs or entity declarations, and are limited to 2,000 items. Every batch is limited to 20,000 observations, and a failed batch is never partly stored.
- The HTML report escapes all text and loads nothing from the network.
- HTTPS certificates are checked by rustls against the Mozilla root certificates built into the program, not against the Windows certificate store. A network that inspects HTTPS with its own root certificate is refused.
- A proxy is used only when `HTTPS_PROXY` is set, and it sees which providers you query. HTTP, SOCKS4A and SOCKS5 proxies receive host names and look them up themselves, so the public-address check is theirs to make. A `socks4://` proxy needs addresses, so CrowdWatch looks names up locally first; use `socks5h://` or `socks4a://` to keep lookups at the proxy, as Tor needs. If the proxy address cannot be used, requests fail instead of connecting directly. Hosts in `NO_PROXY` are reached directly, with the public-address check.
- `verify-skeletrace` runs the SkeleTrace program you name, or the first `skeletrace` on your `PATH`. On Windows the current folder is searched first, as Python's `shutil.which` did. Pass `--host` with a full path when in doubt.
- CrowdWatch's own code contains no `unsafe` Rust; the crate forbids it. Dependencies are pinned in `Cargo.lock`, and SQLite is compiled from the bundled copy in `rusqlite`. Update dependencies deliberately and run the checks before distributing a build.

No formal external security audit has been performed.
