{pkgs}: {
  simple = pkgs.callPackage ./simple {inherit pkgs;};
  cli = pkgs.callPackage ./cli {inherit pkgs;};
  performance = pkgs.callPackage ./performance {inherit pkgs;};
  low-session-bandwidth = pkgs.callPackage ./low-session-bandwidth {inherit pkgs;};
  # complex = pkgs.callPackage ./complex {inherit pkgs;};
}
