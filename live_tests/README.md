# Live tests

This subproject builds a single binary that runs some integration tests with
real caldav servers.

These started out as unit tests, but there's a few goals with don't fit in
within Rust's unit tests model. In particular, test output really need to be
modelled as:

- Passed
- Skipped (missing server support)
- Failed

For example, if a server does not support creating collections, we cannot test
deleting collections either, so that second test is skipped (not failed).

# Status

This works mostly to count how many tests pass, but further refactoring is
required to properly model skipped tests and to properly count the total.

# Running these tests

To run them with `xandikos`, you can start a test server with:

```
docker run --rm --publish 8000:8000 xandikos \
  xandikos -d /tmp/dav -l 0.0.0.0 -p 8000 --autocreate --dump-dav-xml
```

And then execute these tests with:

```sh
cargo run -p live_tests -- live_tests/xandikos.profile
```

Check the shipped `.profile` files for a reference on their format.

Test clients use the discovery bootstrapping mechanism, do you can specify your
providers main site as URL as `CALDAV_SERVER` and DNS discovery should resolve
the real server and port automatically.

DO NOT use the credentials for real/personal/work account for test; these is no
guarantee that these tests won't delete your data!
