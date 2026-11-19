self:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.obojeno;
  toml = pkgs.formats.toml { };
  configFile = "${config.xdg.configHome}/obojeno/config.toml";
in
{
  options.programs.obojeno = {
    enable = lib.mkEnableOption "obojeno, wallpaper-driven theming";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "obojeno.packages.\${system}.default";
      description = "The obojeno package.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = lib.literalExpression ''
        {
          scheme = "tonal-spot";
          wallpaper_dirs = [ "~/Pictures/Wallpapers" ];
          transition = { style = "grow-circle"; duration = 0.8; };
          targets.kitty = { };
          targets.simple-bar.base = "~/dotfiles/.simplebarrc";
        }
      '';
      description = ''
        Written to {file}`~/.config/obojeno/config.toml` on every activation, as a
        writable file: changes made in the menu bar app last until the next
        switch. Leave empty to manage the file yourself.
      '';
    };

    startAtLogin = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Run the daemon at login, restarting it if it crashes.";
    };

    integrations = {
      starship = lib.mkEnableOption ''
        pointing STARSHIP_CONFIG at obojeno's merged starship config (needs the
        `starship` target)
      '';
      herdr = lib.mkEnableOption ''
        pointing HERDR_CONFIG_PATH at obojeno's merged herdr config (needs the
        `herdr` target)
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];

      launchd.agents.obojeno = lib.mkIf cfg.startAtLogin {
        enable = true;
        config = {
          # The label `obojeno service` uses too, so it sees this agent.
          Label = "io.github.st1vc3.obojeno";
          ProgramArguments = [ "${cfg.package}/bin/obojeno" "daemon" ];
          RunAtLoad = true;
          # Restart after crashes, not after a clean exit (another daemon
          # already running).
          KeepAlive.SuccessfulExit = false;
          ProcessType = "Interactive";
          StandardOutPath = "${config.home.homeDirectory}/Library/Logs/obojeno.log";
          StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/obojeno.log";
        };
      };
    }

    (lib.mkIf (cfg.settings != { }) {
      # A copy, not a store link, so the menu bar app can still edit it.
      home.activation.obojenoConfig = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
        $DRY_RUN_CMD mkdir -p ${lib.escapeShellArg (builtins.dirOf configFile)}
        $DRY_RUN_CMD rm -f ${lib.escapeShellArg configFile}
        $DRY_RUN_CMD install -m 0644 ${toml.generate "obojeno-config.toml" cfg.settings} ${lib.escapeShellArg configFile}
      '';
    })

    (lib.mkIf cfg.integrations.starship {
      # programs.starship points STARSHIP_CONFIG at its own file.
      home.sessionVariables.STARSHIP_CONFIG = lib.mkForce "${config.xdg.cacheHome}/obojeno/starship.toml";
    })

    (lib.mkIf cfg.integrations.herdr {
      home.sessionVariables.HERDR_CONFIG_PATH = "${config.xdg.cacheHome}/obojeno/herdr.toml";
    })
  ]);
}
