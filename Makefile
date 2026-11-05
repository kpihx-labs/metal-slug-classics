.PHONY: check doctor build-core build-core-windows build-cores push

check:
	@cargo fmt --check
	@cargo clippy --all-targets --all-features -- -D warnings
	@cargo test --all-features
	@cargo build --release
	@shellcheck scripts/*.sh
	@./scripts/doctor.sh
	@cargo run --quiet -- doctor

doctor:
	@./scripts/doctor.sh
	@cargo run --quiet -- doctor

build-core:
	./scripts/build-core.sh linux

build-core-windows:
	./scripts/build-core.sh windows

build-cores:
	./scripts/build-core.sh all

push:
	git push github main
