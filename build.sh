#!/bin/bash

#~/librespot/build.sh cargo build --release --target mipsel-unknown-linux-musl

~/librespot/build.sh cargo +nightly build --release \
    -Z build-std=std,panic_abort \
    -Z build-std-features=panic_immediate_abort \
    --target mipsel-unknown-linux-musl
