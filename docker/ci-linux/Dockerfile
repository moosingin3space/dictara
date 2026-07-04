# CI image for Linux Rust builds (clippy/tests) of the Tauri backend.
#
# Rust itself is deliberately NOT baked in here — it's installed per-job via
# dtolnay/rust-toolchain, so toolchain/version changes in the workflow don't
# require rebuilding this image. Only the system/apt dependency list below
# should trigger a rebuild.
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    wget \
    file \
    git \
    build-essential \
    pkg-config \
    clang \
    libclang-dev \
    cmake \
    libwebkit2gtk-4.1-dev \
    libxdo-dev \
    libssl-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libasound2-dev \
    libdbus-1-dev \
    # Vulkan headers/loader + glslc: build the whisper.cpp GGML Vulkan backend
    libvulkan-dev \
    glslc \
    && rm -rf /var/lib/apt/lists/*

# Node.js 22 (frontend build + Tauri CLI, used by the Flatpak deb build).
# Ubuntu 24.04's nodejs (18.x) is too old for vite 7.
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*
