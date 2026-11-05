.PHONY: check doctor build-core push

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
	./scripts/build-core.sh

push:
	git push github main
