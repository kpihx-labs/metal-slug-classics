.PHONY: check doctor push

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

push:
	git push github main
