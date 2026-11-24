#!/usr/bin/env bash
# rustc for the packaged macOS build (build-app.sh sets RUSTC_WRAPPER to this): every crate is
# compiled so that its functions unwind by the linker's compact table (__unwind_info), not by a
# DWARF entry each in __eh_frame.
#
# Without it 79 % of the exe's functions (47,212 of 59,537 in a fat-LTO build, 125,183 of 145,052
# in a thin one) carry a DWARF entry: 3.7 MiB and 9.2 MiB of the exe, which macOS reads before a
# first run like every other byte. Two defaults cause it, and each flag below lifts one:
#   -Zuse-sync-unwind=yes   rustc asks for unwind rows at every instruction ("async"), epilogues
#                           included, which the compact form cannot say. Rows at calls are what a
#                           panic needs; clang builds arm64 macOS code this way.
#   -Cllvm-args=--emit-compact-unwind-non-canonical
#                           LLVM keeps the compact form from functions whose landing pads use a
#                           personality other than C++'s or Objective-C's, so from every Rust
#                           function with a destructor on its stack. The linker takes up to three
#                           personalities; the exe has two.
# Panics still unwind and are caught (catch_unwind, a thread's join); crash reports and samplers
# walk frame pointers and the compact table.
#
# -Z needs RUSTC_BOOTSTRAP. It is set for cargo's own compile units only (they carry
# `-C metadata=`), never for the probes build scripts run to find out whether the compiler is a
# nightly: those must keep answering "stable", or crates switch to unstable code paths.
rustc=$1
shift
unit=0
previous=
for arg in "$@"; do
  case "$previous $arg" in "-C metadata="*) unit=1 ;; esac
  case "$arg" in -Cmetadata=*) unit=1 ;; esac
  previous=$arg
done
[ "$unit" = 1 ] || exec "$rustc" "$@"
RUSTC_BOOTSTRAP=1 exec "$rustc" "$@" -Zuse-sync-unwind=yes -Cllvm-args=--emit-compact-unwind-non-canonical
