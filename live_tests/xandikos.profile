# vim ft=toml
# docker run --rm --publish 8000:8000 xandikos xandikos -d /tmp/dav -l 0.0.0.0 -p 8000 --autocreate --dump-dav-xml
host = "http://localhost:8000"
username = "test"
password = "test"

[xfail]
test_create_and_fetch_resource_with_weird_characters = "https://github.com/jelmer/xandikos/issues/253"
