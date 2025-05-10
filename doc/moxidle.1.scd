moxidle(1)

# NAME

moxidle - Configurable Wayland idle daemon with audio inhibition and conditional listeners

# SYNOPSIS

*moxidle* [options...]
*moxidle* *-c* <path>
*moxidle* *-v* | *-q*
*moxidle* *-h* | *-V*

# DESCRIPTION

*moxidle* is a feature-rich idle daemon for Wayland compositors. It goes beyond basic idle management by offering configurable and extensible idle handling, including:

- **Audio Inhibition:** Prevents the system from becoming idle when audio is actively playing, ensuring uninterrupted playback.
- **Conditional Listeners:** Allows you to define different idle listeners based on various conditions (e.g., user activity, specific applications running).
- **Extensible Configuration:**  Configuration through a dedicated file, allowing for detailed customization of idle behavior.

*moxidle* implements the *ext-idle-notify-v1* Wayland protocol, making it compatible with Wayland compositors supporting this extension. It aims to provide a flexible and powerful solution for managing system idle in Wayland environments.

# OPTIONS

*Generic Program Information*

*-h, --help*
	Display help information and quit. Useful for quickly checking available options.

*-V, --version*
	Display version information and quit. Shows the current version of *moxidle*.

*General Settings*

*-c, --config* _path_
	Specifies the path to the configuration file. This file defines the idle behavior and other settings. If not provided, *moxidle* will look for a default configuration file (see FILES section).

*Logging Options*

*-q, --quiet*
	Lower the log level, reducing verbosity of output. Can be specified multiple
	times to further decrease verbosity.

*-v, --verbose*
	Increase the log level, making the output more verbose. Can be specified multiple times to further increase verbosity.
