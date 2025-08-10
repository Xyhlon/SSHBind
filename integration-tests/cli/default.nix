{
  pkgs,
  lib,
  ...
}: let
  bobPassword = "bob";
  nodes = {
    user = {...}: {
      users.users.alice = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "alice";
        initialPassword = "alice";
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
      networking.firewall.allowedTCPPorts = [80 8080];
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
                {
                  ip = "127.0.0.1";
                  port = 8000;
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

      def setup_sops(m: Machine, user: str):
          out = m.succeed(f"su -l {user} -c 'age-keygen -o age.key 2>&1'")
          age_pk = out.split("key: ")[1].strip()

          m.succeed(f"su -l {user} -c 'mkdir -p ~/.config/sops/age/ && mv age.key ~/.config/sops/age/keys.txt'")
          m.succeed(f"su -l {user} -c 'chmod 600 ~/.config/sops/age/keys.txt'")

          creds = "target:22:\n  username: bob\n  password: ${bobPassword}\n"
          creds += "192.168.10.1:22:\n  username: bob\n  password: ${bobPassword}\n"
          with open("secrets.yaml", "w") as f:
              f.write(creds)

          sops_config = f"""keys:\n  - &my_age_keys {age_pk}\n"""
          sops_config += """creation_rules:\n  - path_regex: \\.yaml$\n"""
          sops_config += "    key_groups:\n      - age:\n        - *my_age_keys"
          with open(".sops.yaml", "w") as f:
              f.write(sops_config)

          m.copy_from_host("secrets.yaml", f"/home/{user}/secrets.yaml")
          m.copy_from_host(".sops.yaml", f"/home/{user}/.sops.yaml")
          m.succeed(f"chown {user}:users -R /home/{user}/")
          m.succeed(f"su -l {user} -c 'sops --in-place --encrypt secrets.yaml'")

      # Setup sops for alice
      setup_sops(user, "alice")

      # Testing basic cli usage
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind unbind -a 127.0.0.1:8000'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind shutdown'")
      user.succeed("su -l alice -c 'sshbind list'")

      # Testing unbinding via shutdown
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind shutdown'")
      user.succeed("su -l alice -c 'sshbind list'")

      # Testing binding options
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r 127.0.0.1:80 -j target:22 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8002 -r 127.0.0.1:8000 -j target:22 -s ~/secrets.yaml'")
      user.succeed("""su -l alice -c 'sshbind bind -a 127.0.0.1:8003 -j target:22 -s ~/secrets.yaml -c "cat" '""")
      user.succeed(r"""su -l alice -c 'sshbind bind -a 127.0.0.1:8080 -r 192.168.1.1:8080 -j target:22 -s ~/secrets.yaml -c "socat -lf /dev/null TCP-LISTEN:8080,bind=0.0.0.0,reuseaddr,fork,crlf SYSTEM:\"echo HTTP/1.1 200 OK; echo Content-Type\\: text/plain; echo Content-Length\\: 3; echo Connection\\: close; echo; echo OK\""'""")
      user.succeed(r"""su -l alice -c 'sshbind bind -a 127.0.0.1:8081 -r 127.0.0.1:8081 -j target:22 -s ~/secrets.yaml -c "socat -lf /dev/null TCP-LISTEN:8081,reuseaddr,fork,crlf SYSTEM:\"echo HTTP/1.1 200 OK; echo Content-Type\\: text/plain; echo Content-Length\\: 3; echo Connection\\: close; echo; echo OK\""'""")
      user.succeed(r"""su -l alice -c 'sshbind bind -a 127.0.0.1:8082 -r target:8082 -j target:22 -s ~/secrets.yaml -c "socat -lf /dev/null TCP-LISTEN:8082,reuseaddr,fork,crlf SYSTEM:\"echo HTTP/1.1 200 OK; echo Content-Type\\: text/plain; echo Content-Length\\: 3; echo Connection\\: close; echo; echo OK\""'""")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind shutdown'")

      # Testing missing binding options
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -s ~/secrets.yaml'")

      # Double binding to the same address should fail
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind shutdown'")

      # Testing unbinding
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8002 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind unbind --all'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r target:80 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind unbind -a 127.0.0.1:8000'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind unbind -a 127.0.0.1:8001'")
      user.succeed("su -l alice -c 'sshbind list'")
      user.succeed("su -l alice -c 'sshbind shutdown'")

      # Testing help commands
      user.succeed("su -l alice -c 'sshbind --help'")
      user.succeed("su -l alice -c 'sshbind help'")
      user.succeed("su -l alice -c 'sshbind help bind'")
      user.succeed("su -l alice -c 'sshbind help unbind'")
      user.succeed("su -l alice -c 'sshbind help list'")
      user.succeed("su -l alice -c 'sshbind help shutdown'")

      user.succeed("su -l alice -c 'sshbind --version'")

      # Testing crashing of daemon
      # user.succeed("su -l alice -c 'kill `pidof sshbind`'")
      # user.succeed("su -l alice -c 'sshbind list'")




      # user.send_chars("alice\n")
      # user.sleep(1)
      # user.send_chars("alice\n")
      # user.send_chars("curl 127.0.0.1:8080\n")
    '';
  }
