#!/usr/bin/env sh
set -eu

mkdir -p dist
build() {
  goos=$1 arch=$2 binary=$3 pkg=$4
  suffix=""
  [ "$goos" = windows ] && suffix=".exe"
  echo "Building dist/$binary-$goos-$arch$suffix"
  CGO_ENABLED=0 GOOS="$goos" GOARCH="$arch" go build -trimpath -ldflags='-s -w' -o "dist/$binary-$goos-$arch$suffix" "$pkg"
}

build linux amd64 cross233-server ./cross233-server
build linux arm64 cross233-server ./cross233-server
build windows amd64 cross233-client ./cross233-client
build windows arm64 cross233-client ./cross233-client
build darwin amd64 cross233-client ./cross233-client
build darwin arm64 cross233-client ./cross233-client
build linux amd64 cross233-client ./cross233-client
build linux arm64 cross233-client ./cross233-client
