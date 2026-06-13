{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rust = pkgs.rust-bin.nightly."2026-05-09".default.override {
            extensions = [ "rust-src" "rust-analyzer" ];
          };
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = [
              pkgs.pkg-config
            ];

            buildInputs = [
              rust
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              # Graphics/windowing libraries required by Bevy/winit/wgpu on Linux
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.libx11
              pkgs.libxcursor
              pkgs.libxrandr
              pkgs.libxi
              pkgs.libGL

              # Audio/input
              pkgs.alsa-lib
              pkgs.udev

              # Vulkan loader
              pkgs.vulkan-loader
            ];

            shellHook = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
                pkgs.wayland
                pkgs.libxkbcommon
                pkgs.libx11
                pkgs.libxcursor
                pkgs.libxrandr
                pkgs.libxi
                pkgs.libGL
                pkgs.alsa-lib
                pkgs.udev
                pkgs.vulkan-loader
              ]}:$PWD/target/debug/deps''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            '';
          };
        });
    };
}
