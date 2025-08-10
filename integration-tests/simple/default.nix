{
  pkgs,
  lib,
  ...
}: let
  bobPassword = "bob";
  charliePassword = "charlie";
  charlieTOTPkey = "J5JSV5ITGUK3QFGYJJMG3AJX2NMUIZFI";
  charlieGoogleAuthenticatorFile = ''${charlieTOTPkey}\n" WINDOW_SIZE 17\n" TOTP_AUTH\n18068930\n51114982\n33966717\n13689864\n24303190\n'';
  charlieTOTPkeyHex = "4f532af5133515b814d84a586d8137d3594464a8";
  charlieOathFile = ''
    HOTP/T30/6  charlie  -  ${charlieTOTPkeyHex}
  '';
  davidPassword = "david";
  baseNodes = {
    # We don't need to add anything to the system configuration of the machines
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
      networking = {
        hostName = "user"; # Define your hostname.
      };
      virtualisation.vlans = [10];
    };

    passwd = {...}: {
      users.users.bob = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "bob";
        initialPassword = bobPassword;
      };
      services = {
        openssh = {
          enable = true;
          settings = {
            PasswordAuthentication = true;
          };
        };
      };

      networking = {
        hostName = "passwd"; # Define your hostname.
      };
      virtualisation.vlans = [10 11];
    };

    totp = {pkgs, ...}: {
      users.users.charlie = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "charlie";
        initialPassword = charliePassword;
      };
      environment.systemPackages = with pkgs; [
        google-authenticator
        oath-toolkit
        vim
      ];

      services = {
        openssh = {
          enable = true;
          settings = {
            KbdInteractiveAuthentication = true;
            PasswordAuthentication = true;
            PubkeyAuthentication = false;
            PermitRootLogin = "no";
          };
          extraConfig = ''
            AuthenticationMethods keyboard-interactive:pam
          '';
        };
      };
      security.pam = {
        services = {
          sshd = {
            googleAuthenticator = {
              enable = true;
            };
            # oathAuth = true;
          };
        };
      };
      environment.etc = {
        "users.oath".text = charlieOathFile;
      };
      networking = {
        hostName = "totp"; # Define your hostname.
      };

      virtualisation.vlans = [11 12];
    };

    target = {...}: {
      users.users.david = {
        isNormalUser = true;
        useDefaultShell = true;
        description = "david";
        initialPassword = davidPassword;
        packages = with pkgs; [
          socat
        ];
      };
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
      virtualisation.vlans = [12];

      networking = {
        hostName = "target"; # Define your hostname.
        firewall.allowedTCPPorts = [80];
      };
    };
  };

  nodeConfigs =
    lib.mapAttrs (
      name: nodeFn:
        nodeFn {
          inherit pkgs lib;
        }
    )
    baseNodes;
  nodeNames = builtins.attrNames nodeConfigs;

  nodeNumber = name: let
    idx0 = lib.lists.findFirstIndex (n: n == name) null nodeNames;
  in
    idx0 + 1;

  # For VLAN X, list all *other* peers on that VLAN
  peersOn = vlan: name:
    builtins.filter
    (peer: peer != name && lib.elem vlan nodeConfigs.${peer}.virtualisation.vlans)
    nodeNames;

  extraHostsFor = name: let
    inherit (nodeConfigs.${name}.virtualisation) vlans;
    entries =
      lib.concatMap (
        v:
          map (
            peer: {
              value = ["host-${nodeConfigs.${peer}.networking.hostName}"];
              name = "192.168.${toString v}.${toString (nodeNumber peer)}";
            }
          ) (peersOn v name)
      )
      vlans;
  in
    builtins.listToAttrs entries;
  nodes =
    lib.mapAttrs (
      name: nodeFn: {
        pkgs,
        lib,
        ...
      }: let
        orig = nodeFn {
          inherit pkgs lib;
        };
      in
        orig
        // {
          networking =
            (orig.networking or {})
            // {
              hosts = extraHostsFor name;
            };
        }
    )
    baseNodes;
in
  pkgs.nixosTest {
    name = "Gateway Machine Target Testing Env";
    inherit nodes;

    testScript = ''
      import json
      start_all()

      for m in machines:
          m.wait_for_unit("multi-user.target")

      for m in machines:
          m.wait_for_unit("network.target")

      for m in machines:
          m.systemctl("start network-online.target")

      for m in machines:
          m.wait_for_unit("network-online.target")

      _vlans: dict[Machine, list[str]] = {}
      for m in machines:
          ifs = json.loads(m.succeed("ip -j addr"))
          lans = []
          for i in ifs:
              if i["ifname"] in ["lo", "eth0"]:
                  continue
              for addr in i["addr_info"]:
                  if addr["family"] == "inet":
                      vlan = addr["local"].split(".")[2]
                      lans.append(vlan)
          _vlans[m] = lans

      # Testing network isolation
      # for m in machines:
      #     for other in machines:
      #         contains_any = any(v in _vlans[other] for v in _vlans[m])
      #         if contains_any and m != other:
      #             m.succeed(f"ping -c 2 host-{other.name}")
      #         else:
      #             m.fail(f"ping -c 2 host-{other.name}")

      def setup_sops(m: Machine, user: str):
          out = m.succeed(f"su -l {user} -c 'age-keygen -o age.key 2>&1'")
          age_pk = out.split("key: ")[1].strip()

          m.succeed(f"su -l {user} -c 'mkdir -p ~/.config/sops/age/ && mv age.key ~/.config/sops/age/keys.txt'")
          m.succeed(f"su -l {user} -c 'chmod 600 ~/.config/sops/age/keys.txt'")

          creds = "host-passwd:22:\n  username: bob\n  password: ${bobPassword}\n"
          creds += "192.168.10.1:22:\n  username: bob\n  password: ${bobPassword}\n"
          creds += "host-totp:22:\n  username: charlie\n  password: ${charliePassword}\n  totp_key: ${charlieTOTPkey}\n"
          creds += "192.168.11.3:22:\n  username: charlie\n  password: ${charliePassword}\n  totp_key: ${charlieTOTPkey}\n"
          creds += "host-target:22:\n  username: david\n  password: ${davidPassword}\n"
          creds += "192.168.12.2:22:\n  username: david\n  password: ${davidPassword}\n"
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

      # Setup 2fa for charlie
      with open(".google_authenticator", "w") as f:
          f.write("""${charlieGoogleAuthenticatorFile}""")
      totp.copy_from_host(".google_authenticator", "/home/charlie/.google_authenticator")
      totp.succeed("chmod 600 /home/charlie/.google_authenticator")
      totp.succeed("chown charlie:users -R /home/charlie/")

      user.send_chars("alice\n")
      user.sleep(1)
      user.send_chars("alice\n")
      # user.send_chars("RUST_BACKTRACE=1 sshbind bind -a 127.0.0.1:8000 -r 192.168.12.2:80 -s ~/secrets.yaml -j 192.168.10.1:22 -j 192.168.11.3:22\n curl 127.0.0.1:8000\n")
      # user.send_chars("sshbind bind -a 127.0.0.1:8003 -r host-target:80 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22\n curl 127.0.0.1:8003\n")
      # user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8003 -r host-target:80 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22'")
      # user.execute("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r 192.168.12.2:80 -s ~/secrets.yaml -j 192.168.10.1:22 -j 192.168.11.3:22'", timeout=5)

      # Testing basic usage
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8000 -r 192.168.12.2:80 -s ~/secrets.yaml -j 192.168.10.1:22 -j 192.168.11.3:22'")
      assert user.succeed("su -l alice -c 'curl 127.0.0.1:8000'") == "Hello from NixOS!\n", "Failed ip chain connection with remote service"
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8001 -r host-target:80 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22'")
      assert user.succeed("su -l alice -c 'curl 127.0.0.1:8001'") == "Hello from NixOS!\n", "Failed hostname chain connection with remote service"
      user.succeed("""su -l alice -c 'sshbind bind -a 127.0.0.1:8002 -r 127.0.0.1:12345 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22 -j host-target:22 -c "socat TCP-LISTEN:12345,fork EXEC:cat"'""")
      assert user.succeed(r"""su -l alice -c "printf 'Hello Nixos!\n' | nc -N -w 3 127.0.0.1 8002" """) == "Hello Nixos!\n", "Failed hostname chain connection with remote service and command"
      user.succeed("""su -l alice -c 'sshbind bind -a 127.0.0.1:8003 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22 -j host-target:22 -c "echo \"Hi\""'""")
      assert user.succeed(r"""su -l alice -c "nc -N 127.0.0.1 8003 </dev/null" """) == "Hi\n", "Failed hostname chain connection with remote service and command using internal pipe"
      user.succeed("su -l alice -c 'sshbind bind -a 127.0.0.1:8004 -r 127.0.0.1:8000 -s ~/secrets.yaml -j host-passwd:22 -j host-totp:22 -j host-target:22'")
      assert user.succeed("su -l alice -c 'curl 127.0.0.1:8004'") == "Hello from NixOS!\n", "Failed hostname chain connection with local service"
    '';
  }
