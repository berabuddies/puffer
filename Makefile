SHELL := /usr/bin/env bash

.PHONY: help build-rust build-tauri build-macos build-release-cef pack-macos build-linux pack-linux pack-linux-local

help:
	@scripts/release.sh help

build-rust:
	@scripts/release.sh build-rust

build-tauri:
	@scripts/release.sh build-tauri

build-macos:
	@scripts/release.sh build-macos

build-release-cef:
	@scripts/release.sh build-release-cef

pack-macos:
	@scripts/release.sh pack-macos

build-linux:
	@scripts/release.sh build-linux

pack-linux:
	@scripts/release.sh pack-linux

pack-linux-local:
	@scripts/release.sh pack-linux-local
