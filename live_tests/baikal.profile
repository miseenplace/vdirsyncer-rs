# vim ft=toml
# docker run --rm --publish 8002:80 whynothugo/vdirsyncer-devkit-baikal
host = "http://localhost:8002"
username = "baikal"
password = "baikal"

[xfail]
CreateAndDeleteCollection = "https://github.com/sabre-io/Baikal/issues/1182"
