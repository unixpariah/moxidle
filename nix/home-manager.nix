{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.moxidle;

  toLua =
    value:
    let
      recurse = v: toLua v;
      generators = {
        bool = b: if b then "true" else "false";
        int = toString;
        float = toString;
        string = s: ''"${lib.escape [ ''"'' ] s}"'';
        path = p: ''"${p}"'';
        null = "nil";
        list = vs: "{\n  ${lib.concatMapStringsSep ",\n  " recurse vs}\n}";
        attrs = vs: ''
          {
            ${lib.concatStringsSep ",\n" (
              lib.mapAttrsToList (k: v: "[${generators.string k}] = ${recurse v}") vs
            )}
          }'';
      };
    in
    if builtins.isAttrs value then
      generators.attrs value
    else if builtins.isList value then
      generators.list value
    else
      generators.${builtins.typeOf value} value;

in
{
  options.services.moxidle = {
    enable = lib.mkEnableOption "moxidle, feature rich idle daemon";
    package = lib.mkPackageOption pkgs "moxidle" { };

    settings = lib.mkOption {
      type =
        with lib.types;
        let
          valueType = nullOr (oneOf [
            bool
            int
            float
            str
            path
            (attrsOf valueType)
            (listOf valueType)
          ]);
        in
        valueType;
      default = { };
      example = lib.literalExpression ''
        {
          general = {
            lock_cmd = "pidof \${pkgs.hyprlock}/bin/hyprlock || \${pkgs.hyprlock}/bin/hyprlock";
            unlock_cmd = "\${pkgs.libnotify}/bin/notify-send 'unlock!'";
            before_sleep_cmd = "\${pkgs.libnotify}/bin/notify-send 'Zzz'";
            after_sleep_cmd = "\${pkgs.libnotify}/bin/notify-send 'Awake!'";
            ignore_dbus_inhibit = false;
          };
          timeouts = [
            {
              condition = "on_battery";
              timeout = 300;
              on_timeout = "\${pkgs.systemd}/bin/systemctl suspend";
              on_resume = "\${pkgs.libnotify}/bin/notify-send 'Welcome back!'";
            }
            {
              condition = "on_ac";
              timeout = 300;
              on_timeout = "\${pkgs.systemd}/bin/loginctl lock-session";
              on_resume = "\${pkgs.libnotify}/bin/notify-send 'Welcome back!'";
            }
          ];
        }
      '';
      description = ''
        moxidle configuration in Nix format that will be converted to Lua.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    xdg.configFile."moxidle/config.lua" = lib.mkIf (cfg.settings != { }) {
      text = ''
        -- Generated by Home Manager
        return ${toLua cfg.settings}
      '';
    };

    systemd.user.services.moxidle = {
      Install = {
        WantedBy = [ config.wayland.systemd.target ];
      };

      Unit = {
        Description = "moxidle idle manager";
        PartOf = [ config.wayland.systemd.target ];
        After = [ config.wayland.systemd.target ];
        ConditionEnvironment = "WAYLAND_DISPLAY";
        X-Restart-Triggers = [ config.xdg.configFile."moxidle/config.lua".source ];
      };

      Service = {
        ExecStart = "${lib.getExe cfg.package} -vv";
        Restart = "always";
        RestartSec = "10";
        Environment = [
          "PATH=${
            lib.makeBinPath (
              with pkgs;
              [
                systemd
                libnotify
              ]
            )
          }"
          "LD_LIBRARY_PATH=${lib.makeLibraryPath [ pkgs.libpulseaudio ]}"
        ];
      };
    };
  };
}
