seto(5)

# NAME
moxidle - configuration file

# DESCRIPTION

*moxidle* is the configuration file for the moxidle idle daemon. This configuration file **uses Lua syntax** for defining its settings.

# CONFIGURATION

This section describes the available configuration options within the *moxidle* configuration file.

*General Settings* \

*general.ignore_dbus_inhibit*
	Whether to ignore DBus inhibit requests. \

	_Type:_ bool \

	_Default:_ false \

	_Example:_ true \

*general.ignore_systemd_inhibit*
	Whether to ignore systemd inhibit requests. \

	_Type:_ bool \

	_Default:_ false \

	_Example:_ true \

*general.ignore_audio_inhibit*
	Whether to ignore audio inhibit requests. \

	_Type:_ bool \

	_Default:_ false \

	_Example:_ true \

*general.lock_cmd*
	Command to run when receiving a dbus lock event (e.g. loginctl lock-session)

	_Type:_ String \

	_Default:_ null \

	_Example:_ pidof hyprlock || hyprlock \

*general.unlock_cmd*
	Command to run when receiving a dbus unlock event (e.g. loginctl unlock-session)

	_Type:_ String \

	_Default:_ null \

	_Example:_ pidof hyprlock || hyprlock \

*general.before_sleep_cmd*
	Command to run when receiving a dbus prepare_sleep event

	_Type:_ String \

	_Default:_ null \

	_Example:_ pidof hyprlock || hyprlock \

*general.after_sleep_cmd*
	Command to run when receiving a dbus post prepare_sleep event

	_Type:_ String \

	_Default:_ null \

	_Example:_ pidof hyprlock || hyprlock \

*Listener settings*

*listeners.conditions*
	Conditions required to be fullfilled for timeout notification to be created.

	_Type:_ Table of conditions

	_Default:_ { }

*listeners.on_timeout*
	Command to run when the timeout duration is reached.

	_Type:_ String

	_Default:_ null

	_Example:_ systemctl suspend

*listeners.on_resume*
	Command to run when activity is detected after timeout has fired..

	_Type:_ String

	_Default:_ null

	_Example:_ notify-send "Welcome back!"

*listeners.timeout*
	The timeout duration in seconds. 

	_Type:_ Number

	_Default:_ null

	_Example:_ 300

# CONDITIONS

Conditions are used to define system states that must be met before timeout rules are activated. They can be combined in logical AND fashion.

*Basic power states*

*on_ac*          System is using AC power

*on_battery*         System is using battery power

*Battery percentage conditions*
(table syntax, requires numeric argument)

*battery_above = <percentage>*
	Condition is true when the battery percentage is strictly above the specified `<percentage>` value, which should be a number between 0 and 100.

	_Example:_ `{ battery_above = 50 }` (True if battery is above 50%)

	_Available arguments:_ `<percentage>` - a numeric value representing battery percentage (0-100).

*battery_below = <percentage>*
	Condition is true when the battery percentage is strictly below the specified `<percentage>` value, which should be a number between 0 and 100.

	_Example:_ `{ battery_below = 20 }` (True if battery is below 20%)

	_Available arguments:_ `<percentage>` - a numeric value representing battery percentage (0-100).

*battery_equal = <percentage>*
	Condition is true when the battery percentage is exactly equal to the specified `<percentage>` value, which should be a number between 0 and 100.

	_Example:_ `{ battery_equal = 100 }` (True if battery is exactly 100%, i.e., fully charged)

	_Available arguments:_ `<percentage>` - a numeric value representing battery percentage (0-100).


*Battery state conditions*

*battery_level = <category>*
	Condition is true when the battery level falls into the specified `<category>`.

	_Example:_ `"battery_level = 'low'"` (True if the battery level is considered "low" by the system)

	_Available arguments:_ `<category>` -  See *BATTERY LEVEL CATEGORIES* section for available categories.

*battery_state = <state>*
	Condition is true when the battery power supply status matches the specified `<state>`.

	_Example:_ `"battery_state = 'discharging'"` (True if the battery is currently discharging)

	_Available arguments:_ `<state>` - See *BATTERY STATES* section for available states.

# BATTERY LEVEL CATEGORIES

These represent system-defined battery level thresholds for use with `battery_level` condition:

*unknown*
	The battery level is unknown.

*none*
	There is no battery present.

*low*
	The battery level is low.

*critical*
	The battery level is critical.

*normal*
	The battery level is normal.

*high*
	The battery level is high.

*full*
	The battery is fully charged.

# BATTERY STATES

These describe the current power supply status for use with `battery_state` condition:

*unknown*
	The battery state is unknown.

*charging*
	The battery is currently charging.

*discharging*
	The battery is currently discharging.

*empty*
	The battery is empty.

*fully-charged*
	The battery is fully charged.

*pending-charge*
	The battery is pending charge.

*pending-discharge*
	The battery is pending discharge.

# EXAMPLE CONFIGURATION

```
return {
  general = {
    lock_cmd = "pidof hyprlock || hyprlock",
    unlock_cmd = "notify-send 'Unlocking'",
    before_sleep_cmd = "notify-send 'Going to sleep'",
    after_sleep_cmd = "notify-send 'Awake!'",
    ignore_dbus_inhibit = false,
    ignore_systemd_inhibit = false,
    ignore_audio_inhibit = false,
  },
  listeners = {
    {
      conditions = { "on_battery", { battery_below = 20 } },
      timeout = 300, -- Idle timeout in seconds
      on_timeout = "systemctl suspend",
      on_resume = "notify-send 'Welcome back!'",
    },
    {
      conditions = { "on_ac" },
      timeout = 300,
      on_timeout = "loginctl lock-session",
      on_resume = "notify-send 'Welcome back!'",
    },
		{
      conditions = { "on_ac" },
      timeout = 900,
      on_timeout = "systemctl suspend",
      on_resume = "notify-send 'Welcome back!'",
    }
  },
}
```
