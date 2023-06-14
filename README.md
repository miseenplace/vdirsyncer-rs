# vdirsyncer

[Source](https://git.sr.ht/~whynothugo/vdirsyncer-rs) |
[Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs) |
[Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel) |
[Chat](irc://ircs.libera.chat:6697/#pimutils)

This repository contains work-in-progress rewrite of `vdirsyncer` in Rust, as
well as crates with associated functionality.

For the original Python implementation see https://github.com/pimutils/vdirsyncer.

# Hacking

## Design considerations

These libraries assume that all etags are valid UTF-8 strings. Any response
that does not match this expectation is considered invalid. As of HTTP 1.1, all
header values are restricted to visible characters in the ASCII range (which
satisfy the expectation).

Initial testing indicates that this is not a problem with any CalDav or CardDav
servers.

## Integration tests

A small integration tests helper program is available as part of this project.
It runs a sequence of tests on a real `CalDav` server. See
`live_tests/README.md` for full details.

## Other test servers

Radicale:

```sh
docker run --rm --publish 8001:8001 whynothugo/vdirsyncer-devkit-radicale
```


Baikal:

```sh
docker run --rm --publish 8002:80 whynothugo/vdirsyncer-devkit-baikal
```

- Cyrus IMAP: Hosted test account by Fastmail.com.
- Nextcloud: Hosted test account.

## Sending patches

Just once, configure the patches list for this repo:

```
git config sendemail.to '~whynothugo/vdirsyncer-devel@lists.sr.ht'
```

Make changes. Run tests. Commit. Then send patches:

```
git send-email COMMIT_RANGE
```

# Credits

Special thanks to the [NLnet foundation] that helped receive financial support
from the [NGI Assure] program of the European Commission in early 2023.

[NLnet foundation]: https://nlnet.nl/project/vdirsyncer/
[NGI Assure]: https://www.ngi.eu/ngi-projects/ngi-assure/

# Licence

Copyright 2023 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only
