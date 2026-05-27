.PHONY: help test install validate-smoke full-default

help:
	@echo "bench/Makefile targets:"
	@echo "  install         pip install -r requirements.txt"
	@echo "  test            run pytest"
	@echo "  validate-smoke  python3 -m bench.run validate bench/fixtures/smoke.yaml"
	@echo "  full-default    python3 -m bench.run full --arms default"

install:
	python3 -m pip install -r requirements.txt

test:
	cd .. && python3 -m pytest bench/tests -v

validate-smoke:
	cd .. && python3 -m bench.run validate bench/fixtures/smoke.yaml

full-default:
	cd .. && python3 -m bench.run full --arms default
