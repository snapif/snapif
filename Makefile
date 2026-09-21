.PHONY: help check test deny fmt stealth

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  %-12s %s\n", $$1, $$2}'

fmt: ## rustfmt check
	cargo fmt --check

test: ## cargo test --locked
	RUSTFLAGS="-D warnings" cargo test --locked

deny: ## cargo deny plus forbidden crates
	cargo deny check
	bash scripts/forbid-deps.sh

stealth: ## empty About, no topics, no README pitch
	bash scripts/assert-stealth.sh snapif/snapif

check: ## fmt, clippy, test, deny, workflow trigger lock
	cargo fmt --check
	RUSTFLAGS="-D warnings" cargo clippy --locked --all-targets -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
	RUSTFLAGS="-D warnings" cargo test --locked
	cargo deny check
	bash scripts/forbid-deps.sh
	python3 scripts/test_workflow_triggers.py
	python3 scripts/test_forbid_deps.py
