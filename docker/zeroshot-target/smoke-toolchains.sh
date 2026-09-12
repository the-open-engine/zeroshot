#!/bin/sh
set -eu

# Run in a read-only image with a fresh writable HOME, fixed PATH and no network.
test "$(id -u)" -ne 0
test "$PATH" = /usr/local/bin:/usr/bin:/bin
test ! -w /usr/local/bin
test ! -e "$HOME"
mkdir -m 0700 "$HOME"
work=$(mktemp -d "$HOME/toolchain-proof.XXXXXX")
trap 'rm -rf "$work"' EXIT
cd "$work"

mkdir native
cat >native/probe.c <<'C'
#include <ffi.h>
#include <openssl/sha.h>

int probe_value(void) {
    ffi_cif cif;
    unsigned char digest[SHA256_DIGEST_LENGTH];
    return ffi_prep_cif(&cif, FFI_DEFAULT_ABI, 0, &ffi_type_sint, NULL) == FFI_OK &&
           SHA256((const unsigned char *)"probe", 5, digest) != NULL ? 42 : 0;
}
C
cat >native/Makefile <<'MAKE'
all:
	$(CC) -Wall -Wextra -Werror -fPIC $(shell pkg-config --cflags openssl libffi) -c probe.c -o $(OUT_DIR)/probe.o
	$(AR) crs $(OUT_DIR)/libprobe.a $(OUT_DIR)/probe.o
MAKE
cat >cpp.cpp <<'CPP'
#include <iostream>
int main() { std::cout << 42 << std::endl; }
CPP
c++ -Wall -Wextra -Werror cpp.cpp -o cpp
test "$(./cpp)" = 42

cat >Cargo.toml <<'TOML'
[package]
name = "target-toolchain-proof"
version = "0.1.0"
edition = "2024"
TOML
cp /rust-toolchain.toml .
cat >build.rs <<'RUST'
use std::{env, process::Command};

fn main() {
    let output = env::var("OUT_DIR").unwrap();
    assert!(Command::new("make").arg("-C").arg("native").status().unwrap().success());
    println!("cargo:rustc-link-search=native={output}");
    println!("cargo:rustc-link-lib=static=probe");
    println!("cargo:rustc-link-lib=crypto");
    println!("cargo:rustc-link-lib=ffi");
}
RUST
mkdir src
cat >src/main.rs <<'RUST'
unsafe extern "C" {
    fn probe_value() -> i32;
}
fn main() {
    assert_eq!(unsafe { probe_value() }, 42);
    println!("rust-native: 42");
}
RUST
rustc --version | grep '^rustc 1\.97\.0 '
rustc -vV | grep '^host: .*unknown-linux-gnu$'
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --offline -- -D warnings
cargo run --offline --quiet
test "$(RUSTUP_HOME="$work/custom-rustup" rustup show home)" = "$work/custom-rustup"
mkdir "$HOME/.rustup"
test "$(rustup show home)" = "$HOME/.rustup"

python3.12 -m venv "$work/venv"
"$work/venv/bin/python" -m pip --version
cat >probe.c <<'C'
#include <Python.h>

static PyObject *answer(PyObject *self, PyObject *args) {
    (void)self;
    (void)args;
    return PyLong_FromLong(42);
}
static PyMethodDef methods[] = {{"answer", answer, METH_NOARGS, NULL}, {NULL, NULL, 0, NULL}};
static struct PyModuleDef module = {
    PyModuleDef_HEAD_INIT, "probe", NULL, -1, methods, NULL, NULL, NULL, NULL
};
PyMODINIT_FUNC PyInit_probe(void) { return PyModule_Create(&module); }
C
cc -Wall -Wextra -Werror -shared -fPIC \
  -I"$(python3.12 -c 'import sysconfig; print(sysconfig.get_path("include"))')" \
  probe.c -o "probe$(python3.12-config --extension-suffix)"
"$work/venv/bin/python" - <<'PYTHON'
import ctypes, sqlite3, ssl, sys, probe
assert sys.version_info[:2] == (3, 12)
assert probe.answer() == 42
print("python-native: 42")
PYTHON

mkdir node-addon
cd node-addon
cat >package.json <<'JSON'
{
  "name": "target-toolchain-proof",
  "version": "1.0.0",
  "private": true,
  "scripts": {
    "build": "c++ -Wall -Wextra -Werror -shared -fPIC -I/usr/local/include/node addon.cpp -o addon.node",
    "test": "node -e \"require('node:assert/strict').equal(require('./addon.node'), 42)\""
  }
}
JSON
cat >addon.cpp <<'CPP'
#include <node_api.h>

static napi_value initialize(napi_env env, napi_value exports) {
    (void)exports;
    napi_value result;
    if (napi_create_int32(env, 42, &result) != napi_ok) return nullptr;
    return result;
}
NAPI_MODULE(NODE_GYP_MODULE_NAME, initialize)
CPP
node -e "require('node:assert/strict').equal(process.versions.node.split('.')[0], '24')"
npm install --offline --ignore-scripts --no-audit --no-fund
npm run build
npm test
printf 'node-native: 42\n'
