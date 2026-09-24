all:
	cargo run -r

test:
	cargo test

alltests:
	cargo test && cargo test -- --ignored
