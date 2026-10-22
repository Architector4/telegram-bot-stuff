#!/bin/sh

set -e
set -x

cargo test || exit 1

for i in */; do
	(
		cd "$i" || return
		cargo build --release
		./deploy.sh
	)
done
