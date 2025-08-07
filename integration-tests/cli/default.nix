{
  pkgs,
  lib,
  ...
}: let
  bobPassword = "bob123";
  nodes = {
    user = {...}: {
      users.users.alice = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "alice";
        initialPassword = "alice123";
        packages = with pkgs; [
          age
          sops
          sshbind
          wget
          socat
          dig
        ];
      };
    };
    target = {...}: {
      users.users.bob = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "bob";
        initialPassword = bobPassword;
        packages = with pkgs; [
          socat
        ];
      };
      networking.firewall.allowedTCPPorts = [80];
      services = {
        openssh = {
          enable = true;
          settings = {
            PasswordAuthentication = true;
          };
        };
        httpd = {
          enable = true;
          virtualHosts = {
            "default" = {
              listen = [
                {
                  ip = "0.0.0.0";
                  port = 80;
                }
              ];
              documentRoot = "/etc/var/www";
              extraConfig = ''
                <Directory "/etc/var/www">
                  Require all granted
                </Directory>
              '';
            };
          };
        };
      };
      environment.etc = {
        "var/www/index.html".text = ''
          <!DOCTYPE html>
          <html>
          <head><title>My Site</title></head>
          <body><h1>Hello from NixOS!</h1></body>
          </html>
        '';
      };
    };
  };
in
  pkgs.nixosTest {
    name = "Testing CLI usage";
    inherit nodes;

    testScript = ''
      start_all()

      for m in machines:
          m.wait_for_unit("multi-user.target")

      for m in machines:
          m.wait_for_unit("network.target")

      # Testing basic cli usage
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
    '';
  }
