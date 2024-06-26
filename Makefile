include .env

.PHONY: help
help:
	@echo 'Usage:'
	@sed -n 's/^##//p' ${MAKEFILE_LIST} | column -t -s ':' | sed -e 's/^/ /'

.PHONY: confirm
confirm:
	@echo -n 'Are you sure? [y/N] ' && read ans && [ $${ans:-N} = y ]


## run/api: run the application
.PHONY: run/api
run/api:
	cargo run --release


## db/psql: connect to the database using psql
.PHONY: db/psql
db/psql:
	psql ${database_url}	