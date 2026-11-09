# Nix guideline compliant 2026-09-09
{
  pkgs ? import <nixpkgs> { },
}:
pkgs.mkShell {
  nativeBuildInputs = [ pkgs.pkg-config ];

  buildInputs = [
    pkgs.gtk3
    pkgs.openssl
    pkgs.webkitgtk_4_1
    pkgs.xdotool
  ];
}
