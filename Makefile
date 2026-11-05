.PHONY: check doctor push

check:
	@shellcheck scripts/*.sh
	@./scripts/doctor.sh

doctor:
	@./scripts/doctor.sh

push:
	git push github main
