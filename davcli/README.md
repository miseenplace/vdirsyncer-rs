# davcli

[Source](https://git.sr.ht/~whynothugo/vdirsyncer-rs) |
[Issues](https://todo.sr.ht/~whynothugo/vdirsyncer-rs) |
[Patches](https://lists.sr.ht/~whynothugo/vdirsyncer-devel) |
[Chat](irc://ircs.libera.chat:6697/#pimutils)

**`davcli`** is a command line tool to interact with CalDav and CardDav
servers. It is mostly a simple interface to `libdav`, and part of the
vdirsyncer project.

# Goals

The main goal of this project is to provide a simple command line interface to
expose simple caldav and carddav operations; essentially equivalents to `ls`,
`cp`, `mkdir`, etc.

This tool is still a work in progress. Currently, only the `discover` command
is implemented.

Additionally, the serves as a simpler interface to test our underlying
`libdav`'s implementation with different server installations.

## Discovery

Discovery can be used to test if a server exposes its context path and calendar
home set correctly:

```console
> DAVCLI_PASSWORD=baikal davcli --base-uri http://localhost:8002 --username baikal discover
Discovery successful.
- Context path: http://localhost:8002/dav.php
- Calendar home set: http://localhost:8002/dav.php/calendars/baikal/
```

Discovery also does DNS resolution based on [rfc6764], so can be used to test
if DNS is correctly configured for a publicly hosted service:

[rfc6764]: https://www.rfc-editor.org/rfc/rfc6764

```console
> DAVCLI_PASSWORD=baikal davcli --base-uri https://fastmail.com --username vdirsyncer@fastmail.com discover
Discovery successful.
- Context path: https://d277161.caldav.fastmail.com/dav/calendars
- Calendar home set: https://d277161.caldav.fastmail.com/dav/calendars/user/vdirsyncer@fastmail.com/
```

Errors should generally be useful (please report an issue if you find an
obscure error where the underlying issue is not clear):

```console
> DAVCLI_PASSWORD=baikal davcli --base-uri https://fastmail.com --username wronguser@fastmail.com discover
Error: error querying current user principal

Caused by:
    0: error during http request
    1: http request returned 401 Unauthorized
```

# Credentials

Passwords must be provided as the environment variable `DAVCLI_PASSWORD`.

# Limitations

Only password-based authentication is implemented at this time.

# Todo

This documentation should move to a man page which can be published. It is
basically stashed away in a git repository right now.

# Licence

Copyright 2023 Hugo Osvaldo Barrera  
Licensed under the EUPL, Version 1.2 only
