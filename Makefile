.PHONY: check doctor

check:
	@shellcheck scripts/*.sh
	@./scripts/doctor.sh

doctor:
	@./scripts/doctor.sh
