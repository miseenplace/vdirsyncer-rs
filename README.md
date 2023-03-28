# neosyncer

[Source](https://git.sr.ht/~whynothugo/vdirsyncer-rs) |
[Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs) |
[Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel) |
[Chat](irc://ircs.libera.chat:6697/#pimutils)

This repository contains an experimental rewrite of `vdirsyncer` in Rust, as
well as crates with associated functionality.

# Contributing

Some tests are marked with `#[ignore]`. These are not run by default because
they rely on a server running. To run them with `xandikos`, you can start a
test server with:

```
docker run --rm --publish 8000:8000 xandikos \
  xandikos -d /tmp/dav -l 0.0.0.0 -p 8000 --autocreate --dump-dav-xml
```

And then execute these tests with:

```sh
export CALDAV_SERVER=http://localhost:8000
export CALDAV_USERNAME=test
export CALDAV_PASSWORD=test

cargo test -- --ignored --test-threads=1
```

Test clients use the discovery bootstrapping mechanism, do you can specify your
providers main site as URL as `CALDAV_SERVER` and DNS discovery should resolve
the real server and port automatically.

DO NOT use the credentials for real/personal/work account for test; these is no
guarantee that these tests won't delete your data!

# Sending patches

Just once, configure the patches list for this repo:

```
git config sendemail.to '~whynothugo/vdirsyncer@lists.sr.ht'
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
