default: check

build:
    cargo build

release:
    cargo build --release

test:
    cargo test

clippy:
    cargo clippy -- -D warnings

fmt:
    cargo fmt

check: fmt clippy test

install: release
    install -Dm755 target/release/pike {{env("DESTDIR", "/usr")}}/bin/pike
    install -Dm644 contrib/pike-daemon.service {{env("DESTDIR", "/usr")}}/lib/systemd/user/pike-daemon.service

clean:
    cargo clean

test-integration *backends:
    ./tests/integration/test-integration.sh {{backends}}

test-dnf:
    ./tests/integration/test-integration.sh dnf

test-apt:
    ./tests/integration/test-integration.sh apt
