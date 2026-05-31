SHELL := /usr/bin/env bash

RELEASE_TAG ?= 0.0.1-alpha
CEF_RELEASE_TAG ?= ct
CHROME_RELEASE_TAG ?= $(CEF_RELEASE_TAG)
SOURCE_GITHUB_REPO ?= berabuddies/puffer
RELEASE_GITHUB_REPO ?= berabuddies/puffer
CEF_GITHUB_REPO ?= berabuddies/ct
CHROME_GITHUB_REPO ?= berabuddies/ct

export RELEASE_TAG
export CEF_RELEASE_TAG
export CHROME_RELEASE_TAG
export SOURCE_GITHUB_REPO
export RELEASE_GITHUB_REPO
export CEF_GITHUB_REPO
export CHROME_GITHUB_REPO

.PHONY: help build-rust build-tauri build-macos build-release-cef build-release-chrome pack-macos build-linux pack-linux pack-linux-local

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

build-release-chrome:
	@scripts/release.sh build-release-chrome

pack-macos:
	@scripts/release.sh pack-macos

build-linux:
	@scripts/release.sh build-linux

pack-linux:
	@scripts/release.sh pack-linux

pack-linux-local:
	@scripts/release.sh pack-linux-local
