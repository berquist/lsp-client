default: coverage

coverage:
    cargo tarpaulin --out Html --out Stdout --out Xml
