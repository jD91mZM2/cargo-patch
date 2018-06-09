with import <nixpkgs> {};
stdenv.mkDerivation {
  name = "cargo-patch";
  nativeBuildInputs = [ cmake pkgconfig ];
  buildInputs = [ curl libssh2 openssl ];
}
