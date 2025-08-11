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
        ];
      };
      environment.systemPackages = with pkgs; [
        btop
        wget
        socat
        dig
        tcpkali
        sockperf
        stress-ng
        wrk2
        oha
      ];
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
          Hello from NixOS!
        '';
      };
    };
  };
in
  pkgs.nixosTest {
    name = "Performance Throughput Stability Testing";
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
      user.succeed("su -l alice -c 'RUST_BACKTRACE=full sshbind bind -a 127.0.0.1:8000 -r 127.0.0.1:80 -j target:22 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'RUST_BACKTRACE=full sshbind bind -a 127.0.0.1:8001 -r 127.0.0.1:80 -j target:22 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'RUST_BACKTRACE=full sshbind bind -a 127.0.0.1:8002 -r 127.0.0.1:8000 -j target:22 -s ~/secrets.yaml'")
      user.succeed("su -l alice -c 'RUST_BACKTRACE=full sshbind bind -a 127.0.0.1:8003 -r 127.0.0.1:8000 -j target:22 -s ~/secrets.yaml'")

      user.succeed(r"wrk2 -t4 -c200 -d10s -R 5000 --latency http://127.0.0.1:8000/ && wrk2 -t4 -c200 -d10s -R 5000 --latency http://127.0.0.1:8001/ && wrk2 -t4 -c200 -d10s -R 5000 --latency http://127.0.0.1:8002/ && wrk2 -t4 -c200 -d10s -R 5000 --latency http://127.0.0.1:8003/ ")
    '';
  }
