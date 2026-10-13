#!/usr/bin/env python3
"""Print Typelune's canonical product metadata (from Cargo.toml) as shell assignments.

Usage: eval "$(python3 scripts/product.py)"
Cargo.toml is the single source of truth: [package] version and
[package.metadata.typelune]. Requires Python 3.11+ (tomllib).
"""
import pathlib
import shlex
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
pkg = cargo["package"]
meta = pkg["metadata"]["typelune"]
values = {
    "VERSION": pkg["version"],
    "PRODUCT_NAME": meta["product_name"],
    "BUNDLE_ID": meta["bundle_id"],
    "EXECUTABLE": meta["executable"],
    "APP_BUNDLE": meta["app_bundle"],
    "MINIMUM_MACOS": meta["minimum_macos"],
    "COPYRIGHT": meta["copyright"],
    "C_HEADER": meta["c_header"],
    "C_PREFIX": meta["c_prefix"],
    "ABI_VERSION": meta["abi_version"],
    "REPOSITORY": pkg["repository"],
    "LIB_NAME": cargo["lib"]["name"],
}
for key, value in values.items():
    print(f"{key}={shlex.quote(str(value))}")
