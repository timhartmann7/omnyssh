# Changelog

All notable changes to OmnySSH are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versions follow [Semantic Versioning](https://semver.org/).

---

## Unreleased

### Bug Fixes
- **Security: a server that offers a host key of another type than the one saved for it is refused.** OmnySSH compared the offered key only with saved keys of the same type. A host saved with its Ed25519 key that suddenly offered only an RSA or ECDSA key was therefore taken for a new host: the key was saved and the login went ahead, password included, and the dashboard did this in the background without anyone connecting. Someone intercepting the connection could get past the saved key this way. Like `ssh`, OmnySSH now refuses any key that matches none of those saved for the host, whatever its type, and says which type is saved. A server that really dropped the type of key it was saved with needs its old key removed once, as `ssh` also asks.
- **Security: the terminal app no longer lets server-supplied names control your terminal.** A file or process name with escape sequences went to the terminal as is, so anyone who can create a file in a folder you browse could retitle the terminal, recolour text or clear the screen. Control characters in anything the app draws now show as �, tabs as spaces, and a file keeps its exact name for transfers.
- **A changed host key names its type and, on Windows, whose list held the old one.** OmnySSH 1.1.3 and earlier kept host keys in `%USERPROFILE%\ssh\known_hosts`, a list that PuTTY and OpenSSH never read. When a server was reinstalled, or a new one took over its address, OmnySSH refused it while PuTTY and PowerShell's `ssh` connected, and nothing said why they disagree. A refusal from that file now says it is the list OmnySSH kept up to 1.1.3. Every refusal also names the type of the key the server offered, ED25519, ECDSA or RSA, and the command that prints that key's fingerprint on the server: a server has one key of each type, and comparing the wrong one shows a mismatch that is not there.
- **Servers from the RHEL/CentOS 6 era connect.** OpenSSH before 5.7 failed with "no common key exchange method". Its only key exchange without SHA-1 is diffie-hellman-group-exchange-sha256, which OmnySSH did not offer. Its only host keys are ssh-rsa and ssh-dss, and it predates the rsa-sha2 signatures, so an RSA key could not log in either. OmnySSH now offers diffie-hellman-group-exchange-sha256 and the ssh-rsa host key after every other method, so a server that supports anything newer negotiates exactly as before. An RSA key signs with SHA-1 only for a server that accepts nothing else: one that lists only ssh-rsa as the signatures it takes, or whose version, OpenSSH before 7.2 or Dropbear before 2020.79, predates that list, as PuTTY and `ssh` before 8.8 sign for it. SHA-1 key exchange, CBC ciphers and DSA keys are still left out.
- **FreeBSD hosts show CPU, RAM, load average and top processes.** A FreeBSD server's card showed only disk and uptime: its `top` rejects the options OmnySSH passed, it has neither `free` nor `/proc`, and its `ps` read the list of columns as a single column. CPU is now measured from the kernel's tick counters over one second, so each update of a FreeBSD host takes a second longer. RAM is read from the kernel's page counts, with free and inactive memory, the buffer cache and the ZFS ARC above its minimum counted as available, as htop counts them, so a server on ZFS does not look nearly full. The kernel's idle process, which `ps` charges with all idle time, is left out of the top processes. Where `/proc/loadavg` is missing, the load average now comes from `uptime`, which also fills it in for macOS hosts in the terminal app's host details. As on any system, an account whose login shell is csh or tcsh still shows no metrics.
- **Servers that take only NIST key exchange or aes128-gcm connect, such as Cisco RoomOS video devices.** A server whose key exchange methods include no curve25519 or Diffie-Hellman group 14/16, or whose only cipher is aes128-gcm, failed with "No common algorithm". OmnySSH now also offers ecdh-sha2-nistp256, -nistp384 and -nistp521 key exchange and the aes128-gcm@openssh.com cipher, which macOS's own `ssh` tries first. A host whose only host key is ECDSA P-384, as on a RoomOS device set to ECDSA, now connects the first time too; before, that key was only accepted once it was saved in `known_hosts`. The new key exchange methods and host key type come after the ones offered before, so a server that already worked keeps its key exchange and host key. This needed a newer release of the SSH library (russh 0.63), and building from source with `cargo install omnyssh` now needs Rust 1.89 or later.
- **A server with no algorithm in common says which kind is missing and what it offers.** The error read "No common algorithm", and a key exchange mismatch even "Unknown algorithm", without saying whether the key exchange, host key, cipher or MAC was the problem, so there was nothing to go on. It now reads, for example, "no common cipher; the server offers aes128-cbc". Servers that offer only methods OmnySSH leaves out on purpose, such as SHA-1 Diffie-Hellman or CBC ciphers, still do not connect.
- **macOS: a host on your local network that fails with "No route to host" says macOS may be blocking it.** macOS keeps an app away from devices on the local network until you allow it, and a blocked connection fails at once with "No route to host (os error 65)", while the router usually stays reachable because it is also your DNS server, so it looked like a fault on that one machine. The error now adds that macOS may be blocking the app and where to allow it: System Settings, Privacy & Security, Local Network, then reopen the app. The desktop app also now carries the explanation macOS shows when it asks for that permission. The terminal app needs the permission for the terminal it runs in; Apple's Terminal has it already.
- **A terminal the server closes right after login stays open and shows why.** A Synology NAS gives a shell only to administrators: any other account logs in, gets DSM's "Permission denied, please try again." and the session ends. `ssh` shows that message, but OmnySSH closed the tab the moment the session ended, so the terminal seemed to vanish right after the password while file sessions to the same NAS worked. In both apps, a terminal tab that showed anything now stays open when the remote side ends its session, including after `exit` or a dropped connection, with "[Connection closed. Press Enter to close this tab.]" under the last output. Enter or Esc closes it, and it takes no other input. A terminal that fails before showing anything still closes, with the reason in the status bar. The desktop app could also drop the last lines a session printed just before it ended; they now always arrive.
- **A snippet with the same tag twice no longer empties the Snippets screen.** Tags typed as `db, db` in a snippet form, or written so into `snippets.toml`, left the desktop app's Snippets screen blank, so the snippet could not even be opened to fix it. The Run dialog broke the same way when two hosts share a name, as two `Host` blocks with one name in `~/.ssh/config` give. It now lists that name once, for the host a run goes to. The host picker and the command palette listed such a name twice, and either row opened the first host; they now list it once too.
- **Sizes in the terminal app's file manager stay in their column next to wide file names.** A name with Chinese, Japanese or Korean characters or emoji, which take two cells each, pushed its size past the panel's edge, so a 14-byte file read `1`. Names are now padded and cut by the cells they take.
- **Closing the host picker with Esc gives the keyboard back to the terminal.** In the desktop app, clicking SFTP or Terminal in the sidebar over an open terminal and then pressing Esc left the keyboard on the sidebar, so typing reached nothing until you clicked in the terminal. The command palette's sidebar button had the same problem, and so did clicking the terminal's own tab. A terminal that finishes connecting while the palette is open, or comes into view just as a passphrase prompt opens, no longer takes the keyboard from it.

---

## 1.1.4 — 2026-09-28

### Bug Fixes
- **macOS: the app from the `.dmg` opens instead of being "damaged".** It carried only the signature the linker puts on the executable, which does not cover the rest of the app, so macOS called a downloaded copy damaged and offered no way to open it. The whole app is now signed, ad hoc, and macOS shows its usual warning for an app it cannot verify: open it once from System Settings, Privacy & Security, Open Anyway. The app is still not notarized. A copy installed with `install.sh` was never affected.
- **A changed host key says so, and names the file to fix.** When a server's key no longer matched the one saved for it, as after reinstalling a VM or when another machine takes over an address, the connection failed with "Unknown server key", which reads as the opposite of what happened and pointed nowhere. It now says the host key has changed, shows the new key's SHA256 fingerprint, names the `known_hosts` file holding the old one and gives the `ssh-keygen -R` command that removes it, to run once you have checked that fingerprint on the server itself. The text on the dashboard card can be selected, so the command can be copied. A `known_hosts` that OmnySSH cannot parse is named in the error too.
- **Windows: host keys are kept in `%USERPROFILE%\.ssh\known_hosts`, shared with OpenSSH.** OmnySSH kept its own list in `%USERPROFILE%\ssh\known_hosts`, without the dot, a folder nothing else uses and nobody thinks to look in, so a key you had already accepted in PowerShell's `ssh` meant nothing to it and a stale one could only be removed there. Keys are now checked against, and new ones saved to, the file Windows OpenSSH uses. Keys saved in the old file are still honoured and never quietly replaced.
- **Host keys are matched the way `ssh` matches them.** A `known_hosts` holding an old and a new line for the same host refused the connection over the old line; one matching line is now enough. When the file knows a host by one key type, OmnySSH asks the server for that type first, so a host saved with only its ECDSA key, as `ssh` before 8.5 and Windows 10's built-in client saved them, is checked against that key instead of being taken as a new one. Host names are matched regardless of case, since `ssh` writes them in lower case.
- **The local file pane can switch drives (Windows).** The desktop app's local pane started in your home folder and could climb no higher than the root of drive C:, so files on any other drive, mapped network drives included, could not be uploaded, and nothing could be downloaded to them. A drive selector next to Upload now lists every drive letter, and the local Refresh button picks up a drive plugged in since. In the terminal app, `d` in the file manager moves the local panel to the next drive. macOS and Linux have a single root and are unchanged.
- **Hosts with an `IdentityFile` log in with that key first, however many keys the SSH agent holds.** With an agent holding many keys, as password managers such as Bitwarden, 1Password and KeePassXC set up, OmnySSH offered them all in the agent's order whatever the host's `IdentityFile` said, and the server hung up after its `MaxAuthTries` (three to six attempts) before the right key came up, with "no key was accepted and no password is saved". Adding a key to the agent could take hosts offline that had worked the day before. The agent key matching the host's `IdentityFile` now goes first, as with `ssh`: the file itself when it names a `.pub`, else the `.pub` beside it, else the public half of the private key, which needs no passphrase. `IdentitiesOnly yes` in a host's block of `~/.ssh/config` limits the login to that key and leaves the default `~/.ssh/id_*` keys out, and it is read from there even for a host you have since edited in OmnySSH. A key the server has already turned down through the agent is not offered again from its file, except an RSA key, which some servers, such as older Dropbear, accept only the way the file offers it. The same change ends the passphrase prompt some of these hosts showed for a key the agent held all along. `IdentityAgent` is still not followed, and `IdentitiesOnly` under `Host *` or `Match` is not read.

---

## 1.1.3 — 2026-09-27

### Features
- **Forward local ports over SSH, like `ssh -L`.** A service that listens only on a server's loopback, or on a machine only that server can reach, such as a web admin panel or a database, can now be reached at a port on your own machine. Add forwards to a host in the host editor of the desktop app, or in the host form of the terminal app as `port:host:hostport` (the `ssh -L` notation), or let OmnySSH read the host's `LocalForward` lines from `~/.ssh/config`. The local port listens on this machine only unless you give it an address such as `0.0.0.0`, and the remote host is resolved by the server, so `localhost` is the server itself. One tunnel per host carries all of its ports, and tunnels on any number of hosts can run at once. Start and stop one from the dashboard card, or with `f` in the terminal app, and a host can be set to start its tunnel when OmnySSH opens. A dropped connection is redialled with a backoff while the local ports stay bound, so no other program takes them in the meantime. A tunnel that cannot log in yet, because its key is locked or no password is known, holds its ports until the key is unlocked or the password is typed in a terminal. A login the server rejects or a changed host key stops the tunnel instead. Hosts behind a `ProxyJump` bastion work the same way. Remote (`-R`) and SOCKS (`-D`) forwarding are not included.
- **OmnySSH asks for the login password when no key gets in.** A host without a working key and without a saved password (typically one imported from `~/.ssh/config`) failed with "SSH authentication failed" and offered no way in short of saving its password to disk. A terminal now asks the way `ssh` does, right in the tab (`user@host's password:`), and a file session asks in a dialog. Three tries, then the login fails; Ctrl+C or Cancel ends it. The password is kept in memory until you quit, never written to disk, and only once the server has accepted it; the dashboard, and any tunnel on that host, pick it up and connect on their own and never ask themselves. The first time OmnySSH meets a server, the prompt shows the host key it just recorded, so you can check it before typing. A saved password is still tried first, and a key whose passphrase is needed still comes before any password when the host names it as its identity file.
- **Copy from the desktop terminal with Ctrl+Shift+C.** On Windows and Linux there was no key that copied: Ctrl+C sent the shell an interrupt, as it must, and the only way to copy was the right-click menu. Ctrl+Shift+C now copies the selection, as in GNOME Terminal and Windows Terminal, and Ctrl+Shift+V keeps pasting. Ctrl+C and Ctrl+V still reach the shell as ^C and ^V, which vim and readline rely on. On macOS Cmd+C and Cmd+V already worked and are unchanged.
- **Forward your SSH agent to a host, like `ssh -A`.** A host can now lend its terminals your local SSH agent, so `sudo` through `pam_ssh_agent_auth`, `git` over SSH and hopping on to another server all work with the keys on your machine, and `SSH_AUTH_SOCK` is set on the remote side. It is off for every host until you turn it on with "Forward SSH agent" in the host form of both apps, or the host has `ForwardAgent yes` in `~/.ssh/config`. Only terminals get the agent, only on the host itself and not its `ProxyJump` bastions, and only while an agent is running here; the dashboard, file sessions and tunnels never do. Anyone with root on that server can use your keys while a terminal is open, so keep it to servers you trust. A server that opens an agent channel it was not offered is refused, as `ssh` does. A `ForwardAgent yes` under `Host *` or `Match`, or one pointing at another agent's socket, is not read, and `IdentityAgent` is not followed; a `ForwardAgent no` anywhere above a host keeps its agent home. Not available on Windows yet, where OmnySSH does not use the agent for logins either.
- **Minimize or close the desktop app to the system tray.** Closing the window quit the app, and with it every open terminal, file transfer and tunnel. Two settings under Window, one for minimizing and one for closing, now keep it running in the tray instead, and the tray icon brings the window back or quits. Both are off until you turn them on. On macOS the icon lives in the menu bar and minimizing stays with the Dock. Launching OmnySSH again while it runs now brings the running window forward rather than starting a second copy. On Linux the tray needs `libayatana-appindicator3` (the `.deb` and `.rpm` now depend on it, and the AppImage carries its own copy) and a panel that shows tray icons, which stock GNOME does not without the AppIndicator extension; where either is missing, the settings say so and the window keeps closing as before. Minimizing to the tray needs X11: Wayland never tells an app its window was minimized.

### Bug Fixes
- **Ctrl keys work in the desktop terminal on a non-Latin keyboard layout (Linux).** With a Russian, Ukrainian, Greek or other non-Latin layout active, Ctrl+C, Ctrl+D, Ctrl+Z and the other Ctrl chords sent nothing to the shell, and Ctrl+Shift+V did not paste, because the Linux webview reports no key code for those letters. The physical key now decides, as in GNOME Terminal. Windows, macOS and the terminal app were not affected.
- **Devices that only take the password by keyboard-interactive log in.** UniFi consoles such as the Dream Machine Pro, and other servers with `PasswordAuthentication no`, accept a password only through keyboard-interactive, which is what `ssh` and PuTTY fall back to without telling you. OmnySSH sent the saved password by the password method alone, so these hosts showed as offline with "SSH authentication failed". It now offers the password the other way too, and remembers which one a server takes, so a wrong password costs one failed login, not two. A server that asks for a one-time code instead of a password is told so rather than sent the password.
- **A silent or refusing SSH agent no longer leaves every host stuck on "connecting".** Every login asks the agent first, and nothing bounded that: an agent that accepts connections but never answers (as the launchd agent does on some macOS Tahoe setups), or one that turns a signature down (a declined 1Password or Secretive approval, a key added with `ssh-add -c`) held the login forever, password hosts included. The agent now gets five seconds to list its keys and a minute to sign, a refused signature moves on to the next method, and a signature you turned down is not asked for again by background reconnects.
- **The dashboard card says why a host is down.** A failed host showed a grey "offline" with the reason nowhere on screen. The card now shows it ("SSH connection failed: Connection refused", an authentication failure, a timeout), hidden in streamer mode like the tunnel's reason. The terminal app keeps the whole cause in the host's detail view, and a terminal that fails to open no longer replaces its reason with "SSH session closed.".
- **`omny -v` no longer writes login passwords to its log.** At debug level the SSH library dumps the first login request it sends, and when a password was the first thing tried, that dump carried it. Those dumps are now kept out of the log whatever `RUST_LOG` asks for.
- **Settings under a `Match` block in `~/.ssh/config` no longer land on the host above it.** OmnySSH does not evaluate `Match` conditions, but it read the lines under one as more settings of the `Host` entry just before it, so a `HostName`, `User`, `Port`, `IdentityFile` or `ProxyJump` meant only for that condition was applied to that host whether or not it matched. `Match` blocks are now skipped, the way wildcard `Host` blocks already were.
- **The Linux AppImage opens on current graphics drivers.** On distributions with a recent Mesa, such as Arch, CachyOS, Bazzite and Nobara, it aborted at launch with `Could not create default EGL display: EGL_BAD_PARAMETER` and never showed a window, and the software-rendering restart added in 1.1.2 ran into the same abort: the failure happens while the graphics driver is being loaded, before WebKit picks a renderer, so no WebKit setting can reach it. The cause was the bundle itself. It carried the build machine's own Wayland and X client libraries and put them ahead of yours on the library path, so your Mesa was loaded against a libwayland older than the one it is built against and a symbol it needs was missing. Those ten libraries are no longer packed into the AppImage; your system's copies are used, as they already were for libX11. The `.deb`, `.rpm`, macOS and Windows builds were never affected.
- **Keys with a passphrase work.** A host whose identity file is encrypted failed with a bare "authentication failed", because the key was always read without a passphrase. Both apps now ask for it, once per key, and keep it in memory until you quit. It is never written to disk. The dashboard and any tunnel waiting on that key reconnect as soon as it is unlocked; a terminal or file session that stopped on it has to be opened again. A wrong passphrase says so, a key changed on disk is asked for again, and hosts behind a `ProxyJump` bastion are asked the same way. Cancelling the prompt keeps it away until you open a terminal or file session that needs the key. The host's Password field stays the login password, not the key passphrase.

---

## 1.1.2 — 2026-08-22

### Features
- **Hosts imported from `~/.ssh/config` can be edited in the desktop app.** Imported hosts were shown read-only, so changing a port or a username for one meant deleting it from your SSH config and adding it again by hand — while the terminal app had allowed the edit all along. Editing one now saves your own copy, which OmnySSH uses from then on; `~/.ssh/config` is still never written to. The form says so before you save, since later changes you make to that file stop reaching the host once it has been adopted. The bastion and key path the app parsed are carried over even though the form cannot show them, so an adopted `ProxyJump` host keeps connecting through its jump server. Deleting is still offered for your own hosts only — there is nothing of an import to remove — and deleting a copy you adopted brings the imported version back, which the confirmation now tells you.
- **Watch a host by a port check instead of an SSH login.** Firewalls, switches and other appliances answer SSH but have no shell to read `top` or `free` from, so monitoring could only ever fail on them — and log in again every cycle to find that out. A host can now be set to **TCP port check**: OmnySSH opens a connection to the port and closes it, with no login and no commands. Its card shows reachable / unreachable in place of the metric tiles, rather than tiles for numbers nobody collected. Set it in the host form — `tcp` (the host's SSH port) or `tcp:PORT` in the terminal app, a dropdown in the desktop app. Existing hosts are untouched and stay on SSH monitoring. ICMP is not offered yet: unprivileged ping is unavailable on the Linux packaging most people use.

### Bug Fixes
- **The dropdowns in the desktop app look like the rest of the app.** The monitoring picker on the host form and the scope picker on the snippet form were plain `<select>` elements, which every platform draws itself: on macOS that is a light grey popup button with a double arrow, sitting on the dark form as a bright hole a good deal shorter than the fields around it, and no styling on the element had any effect on it. Windows and Linux drew their own versions of the same mismatch. Both now use one control that strips the platform chrome and matches the text fields exactly, in either theme. The list that opens is still the operating system's, so keyboard, screen readers and touch behave as they always did.
- **Nerd Font glyphs render instead of empty boxes.** The desktop terminal asked for a list of system monospace fonts, none of which carry the glyphs that prompts like starship and powerlevel10k, or `eza --icons`, draw from — so those came out as boxes even for people who had a Nerd Font installed. Common Nerd Font families are now named as fallbacks, behind the regular monospace ones so ordinary text keeps the same typeface and spacing. Snippet output and SFTP file previews get the same treatment, since both show text straight from the server. You still need a Nerd Font installed; the app does not ship one.
- **The desktop app reopens at the size and position you left it.** Every launch reset the window to 1100x720 wherever the system chose to put it, so a window sized to your screen — or moved to a second monitor — had to be set up again each time. Size and position now persist. Visibility deliberately does not: the window still starts hidden and appears once the interface has painted, so restoring geometry never brings back the blank frame at launch.
- **macOS: the window buttons no longer sit across the edge of a collapsed sidebar.** The red/amber/green cluster is positioned by the system against the window, not by the app's layout, and it is wider than the collapsed sidebar was — so the sidebar's edge fell on the green button and left it over the content area while the other two stayed on the sidebar. The collapsed sidebar is now wide enough to hold the whole cluster on macOS. Windows and Linux draw their own title bar and keep the narrower one.
- **Hosts split across `Include` files are imported.** A relative pattern — `Include conf.d/*.conf`, the form nearly every split-config guide prints — was looked for in whatever directory the app happened to be launched from, which for the desktop app started from Finder or the application menu is `/`. Nothing matched, so every host defined in `~/.ssh/conf.d` was missing and had to be added by hand. Those patterns now resolve against `~/.ssh`, the way `ssh` itself resolves them. Four things in the same code path were fixed alongside it: full glob patterns work (`?`, `[abc]`, a wildcard in a directory name, and more than one `*` in a file name), several pathnames on one `Include` line are all read instead of none, a quoted path keeps its spaces, and an `Include` written inside a `Host` block no longer swallows that host's remaining settings. An `Include` that matches nothing is written to the log rather than passing in silence.
- **CPU no longer reads ~91–100 % on an idle server whose system language isn't English.** Monitoring reads the idle percentage from `top` and shows the rest as used. On a server set to a language with a comma decimal separator, `top` prints `99,1 id`, and the parser split that line on commas — reading the idle value as `1` and reporting 99 % used on a machine doing nothing. The same servers could show RAM and Disk as N/A, because `free` and `df` translate the labels being looked for, and the process list could come back empty. The monitoring commands now run in a fixed locale, and the CPU parsers understand a decimal comma on their own for hosts where that cannot be set. Process names keep the server's own character set.
- **A host that answers SSH but not shell commands is no longer re-logged-in every 30 seconds.** Network appliances — firewalls, switches — authenticate fine but cannot run `top` or `free`, and a round of failed metric commands was treated as a dead connection. The retry delay was reset on every successful login, so it never grew past its first step: one login every 30 seconds, indefinitely, and as often as every 10 seconds in the desktop app, whose refresh timer also cut the retry delay short. The delay now escalates properly and is left alone by the refresh timer — the trade-off being that a manual refresh no longer cuts a retry delay short, so a host that has been unreachable for a while retries on its own schedule (at most five minutes). Separately, a device that ignores SSH keepalives was being disconnected after 30 seconds even while its commands still worked; connections are now held open, with liveness still bounded by the keepalive limit.
- **`ProxyJump` hosts now connect through their bastion.** A host with `ProxyJump` in `~/.ssh/config` was parsed but never routed: the terminal refused to open it, while metrics, SFTP, snippets and key setup quietly dialled the target address direct — which for an internal host meant every connection failed or, worse, landed somewhere else on that address. Every native SSH path now walks the jump chain the way `ssh -J` does, connecting and authenticating each bastion in turn and tunnelling the next hop over it.
  - The jump alias is resolved against your host list, so `ProxyJump public-proxy` picks up that entry's `HostName`, `User`, `Port` and `IdentityFile`. An alias that matches no entry is used as a literal hostname.
  - Multi-hop values (`ProxyJump first,second`), inline `user@host:port` overrides, IPv6 literals, bastions that are themselves behind a bastion, and the `ProxyJump none` opt-out all behave as OpenSSH does — including its precedence rule that only the first hop of a list contributes bastions of its own. Chains that loop, or run past ten hops, are reported instead of hanging.
  - A `ProxyJump` that cannot be resolved — a malformed hop, an unreadable host list, a cycle — fails the connection. It never falls back to dialling the target address, which is the very thing that made the old behaviour dangerous.
  - Each hop's host key is checked against `known_hosts` under its own name, and each hop authenticates with the usual agent → identity file → default keys → password order. Every hop, including the tunnel opened on a bastion, is bound by the same ten-second budget, so a firewalled target cannot leave a host stuck on "connecting".
  - A bastion you renamed after importing it is still found by the alias other entries name it with.
  - One-click SSH key setup works for hosts behind a bastion: its verification steps now get the time the longer connection needs, and running out of time after password authentication has been disabled rolls the server back instead of reporting a clean failure.
  - Editing an imported host in the TUI no longer drops its `ProxyJump`, nor the name it was imported under: the form has no field for either, so the saved copy used to lose the bastion, and a second edit lost the imported name — which brought the `~/.ssh/config` entry back as a duplicate card and left any other host's `ProxyJump` pointing at an alias nothing answered to. The GUI already preserved both.
- **Windows: the console-window fix from 1.1.1 actually reaches you this time.** The fix was in that release's source, but the Windows installer published under the `v1.1.1` tag was an artifact left over from an earlier build — so the `.exe` it installed was the 1.1.0 one, still linked as a console application, and none of 1.1.1's Windows fixes (the console window, the white launch flash, the `ssh-keygen` console) were in it. The release now discards anything a previous build left behind, publishes only the bundle it just produced for the version being released, and refuses to publish at all if the desktop binary is not linked as a GUI application or if any installer is missing.
- **The update banner links to the release instead of offering an install that cannot run.** Self-update has no endpoints configured yet, so the Install button reported "Self-update is not available yet" on every platform. It now opens the release page in your browser; Install returns when the desktop updater is wired up.
- **A Linux desktop build that opens to an empty window restarts itself once.** On some graphics setups WebKit cannot start its hardware renderer, so the app opened as an empty dark frame and stayed that way, with the error going to a terminal nobody launched it from. If the interface still has not loaded twelve seconds in, OmnySSH now restarts itself once with software rendering. This covers the `.deb` and `.rpm` as well as the `.AppImage`: the failure is WebKit meeting a particular graphics driver rather than anything the AppImage does, and `install.sh` now prefers the `.rpm` on the distributions where it is reported most, so leaving the native packages out would have moved the affected people onto the one build with no answer for it. A launch that loads normally is untouched, a launch you started with `WEBKIT_DISABLE_DMABUF_RENDERER` set yourself is left alone, and the restart can happen only once. macOS and Windows are unaffected.
- **Editing a host no longer drops every other host's monitoring session.** Putting the monitoring mode on the host form meant the terminal app had to restart its polling pool after an edit, since a running poller does not pick up a changed address, port or mode. It restarted the whole pool for any edit at all, so fixing a typo in one host's notes reconnected and re-authenticated every server on the dashboard. The pool is now restarted only when the edit moved something a connection is actually made from — the name, address, user, port, key, password or monitoring mode. Editing tags or notes leaves every session where it was.
- **macOS: `install.sh` no longer installs an older copy of the app.** The installer read the mount point out of `hdiutil`'s output by stopping at the first space. When a volume called OmnySSH was already mounted — because you had opened the `.dmg` in Finder first, or because an earlier run was interrupted — macOS mounts the new one as `OmnySSH 1`, and the truncated path pointed back at the one already there. The script then copied that older app into `/Applications` and reported success.
- **Linux: a `.deb` that could not be installed is no longer reported as installed.** `apt-get install -f` resolves an unsatisfiable dependency by removing the package `dpkg` has just unpacked, and exits successfully having done so — which the installer read as a working install and announced as one, leaving you with the message and nothing else. It now asks `dpkg` what is actually installed, and falls back to the AppImage when the package did not survive, the way the `.rpm` path already did.
- **Linux: installing the `.deb` removes the AppImage it replaces.** The `.rpm` path already did this and the `.deb` path did not. The package installs to `/usr/bin` under its own name while an AppImage from an earlier run sits in `/usr/local/bin`, which comes first on the default `PATH` — so `omnyssh` on the command line kept starting the old build, and the application menu carried two identical entries.
- **ARM64 Linux and Termux install the terminal app instead of nothing.** No desktop build exists for those targets, and `curl … | sh` — the command the README prints — installs the desktop app by default. It warned and exited without installing anything, although the terminal app ships for exactly those targets. It now installs that instead. A desktop build that does exist and fails to install still fails rather than quietly falling back.

### Packaging
- **A native `.rpm` for Fedora and other RPM distributions.** Releases now carry `OmnySSH-x86_64.rpm` alongside the `.AppImage` and `.deb`, and `install.sh` prefers it on `dnf`-based systems — it lands in your application menu and uninstalls with `dnf remove`, no FUSE involved. The package names the WebKitGTK 4.1, JavaScriptCore and GTK 3 libraries it links, so `dnf` resolves them from your own distribution instead of the app carrying a second copy. Distributions that ship no WebKitGTK 4.1 at all, such as RHEL 9 and its rebuilds, will refuse the package; `install.sh` then tries the AppImage there.
- **The Linux packages are named `omny-ssh`.** The bundler derives the package name from the product name, so the `.deb` and `.rpm` register as `omny-ssh` even though the files are called `OmnySSH-x86_64.deb` and `OmnySSH-x86_64.rpm`. Uninstall with `dnf remove omny-ssh` or `apt remove omny-ssh`.

---

## 1.1.1 — 2026-07-28

### Bug Fixes
- **Windows: the desktop app no longer opens a console window next to itself.** The GUI was linked as a console application, so Windows gave it a terminal and kept it on screen for the whole session. Release builds now link as a GUI binary. The same fix would have made one-click SSH key setup flash a console of its own while `ssh-keygen` runs, so that is suppressed too. macOS and Linux were never affected.
- **No more white flash when the desktop app launches.** The window used to appear before the webview had painted anything, so the first frame was blank white before the dark (or light) interface took over. The window is now created hidden and revealed once the page is up, with a fallback that shows it anyway if the interface is slow to load.

---

## 1.1.0 — 2026-07-24

### Features
- **OmnySSH Desktop — a new native GUI app.** A desktop application built on the same engine as the TUI, sharing your `hosts.toml` and `snippets.toml`. It bundles a live metrics dashboard, a multi-session terminal, an SFTP file manager, command snippets, host management, one-click SSH key setup, a command palette, and light/dark themes. Distributed as native installers (macOS `.dmg`, Linux `.AppImage`/`.deb`, Windows `.exe`). The TUI is unchanged and still installs the same ways. macOS builds are unsigned — on first launch right-click the app and choose **Open** (or install via `install.sh`, which sidesteps the Gatekeeper prompt).
- **File manager hidden-file toggle (`.`)**: show or hide dot-prefixed entries (hidden by default). The `..` parent entry is always shown.
- **Nerd Font file icons in the file manager**: per-type glyphs replace the `[DIR]`/`[   ]` markers. Requires a Nerd Font — without one the glyphs render as empty boxes.

### Changed
- **Terminal next-tab moved from `Tab` to `Ctrl+N`**, freeing `Tab` for the shell's own completion inside the embedded terminal.
- **File manager `h`/`Left` now navigates to the parent directory** (previously moved the cursor up), matching `ranger`/`lf`/`nnn`/`vifm`. Returning to a parent places the cursor on the directory just left.

### Bug Fixes
- **The terminal now handles Cyrillic and other multibyte text.** OmnySSH forwards a UTF-8 locale (and the `IUTF8` PTY mode) to the remote shell, mirroring a normal ssh client's `SendEnv LANG LC_*`. Previously the session could fall back to a single-byte locale, so line editing corrupted the prompt on Cyrillic input and editors like `vim` rendered mojibake (plain `cat` was fine). Applies to both the TUI and GUI terminals. Best-effort — servers that don't accept forwarded env vars are unaffected.

### Packaging
- **Release builds now ship the GUI.** CI bundles native desktop installers for macOS (arm64 + x86_64), Linux x86_64 (`.AppImage` + `.deb`), and Windows x86_64, attached to each GitHub Release alongside the TUI archives and covered by `SHA256SUMS`.
- **`install.sh` can install the GUI, the TUI, or both** via `--gui` / `--tui` / `--both` (or `OMNYSSH_INSTALL=…`); it prompts when run interactively and defaults to the GUI (the flagship app) for piped `curl | sh`.

### Documentation
- **CONTRIBUTING** now documents the `omnyssh-gui` crate and how to build/test the GUI with the Node/Tauri toolchain.
- Documented the Auto SSH Key Setup feature (`Shift+K`): added a README Features entry, a Quick Start key reference, and a Help popup shortcut. The feature already existed but was undiscoverable.
- README "Development Roadmap" now lists the current `1.1.0` release (and `1.0.5`); it previously stopped at `1.0.4`.
- Removed the dead `connect` keybinding from the `config.toml` example and the `[keybindings]` config struct. Enter-to-connect was always hard-coded, so the field never had any effect.

### Internal
- **Repository converted to a cargo workspace; the engine now lives in its own crate**: The SSH engine, host/snippet/app configuration, metrics parsers, domain events, and the self-updater moved into the new `omnyssh-core` library crate (`crates/omnyssh-core`), which has no dependency on terminal-UI or CLI crates. The TUI application keeps the `omnyssh` package name and the `omny` binary (`crates/omnyssh`) and consumes the core as a regular dependency. This prepares the architecture for additional frontends (e.g. a GUI) without duplicating the engine.
  - The event bus is split: background tasks emit `CoreEvent` values; the TUI wraps them into its own `AppEvent` stream alongside input events.
  - `threshold_color` was replaced by a UI-agnostic `ThresholdLevel` in the core; the colour mapping moved to the TUI theme module.
  - Keybinding parsing (config strings → key codes) and PTY key-to-bytes translation moved from the engine into the TUI crate; the core PTY API accepts raw bytes only.
  - Removed the unused `nucleo` dependency.
  - No user-facing changes: behavior, appearance, the binary name, and install paths are unchanged.
- **Removed leftover Alerts/Deep Probe remnants**: Deleted the unused `[smart_context]` config (`SmartContextConfig` was parsed but never read) and reworded stale "Deep Probe" comments left after the subsystem was removed in 1.0.5. No user-facing change — existing config files containing a `[smart_context]` section still load, as the section is ignored.

---

## 1.0.5 — 2026-06-07

### Bug Fixes
- **docs.rs documentation build fixed**: The build script wrote the generated man page into the source tree (`doc/omny.1`), which fails on docs.rs because it mounts the sources read-only. The man page is now skipped when building on docs.rs (detected via the `DOCS_RS` environment variable); normal builds still generate and check it in as before.

### Internal
- **Dead-code cleanup**: Removed the unimplemented Alerts/Deep Probe subsystem (backend and UI) and a range of unused items — enum variants, struct fields, helpers, and theme colors. No user-facing behavior changes: METRICS and SERVICES on the detail page, the file manager, and the terminal work as before. The crate-level `#![allow(dead_code)]` was dropped so the compiler now catches dead code on its own.

---

## 1.0.4 — 2026-05-29

### Features
- **Terminal now uses the native russh client (fixes the dead Windows terminal)**: The interactive terminal no longer spawns the system `ssh` binary inside a local pseudo-console (ConPTY on Windows). It now runs over the same pure-Rust russh client as metrics and SFTP, so the terminal works identically on every OS — the blank, unresponsive Terminal tab on Windows is fixed. The terminal reuses the app's existing key, SSH agent, password, and `known_hosts` (trust-on-first-use) authentication, so password-auth hosts still connect without prompting and the separate `SSH_ASKPASS` helper has been removed.
  - ProxyJump (`-J`) is not yet supported in the terminal and is refused with a clear message instead of connecting direct; metrics/SFTP never used it. Tracked as a follow-up.
  - Passphrase-protected keys without an SSH agent cannot be unlocked interactively in the terminal anymore. Use an SSH agent or an unencrypted key. On Windows there is no agent fallback yet, so prefer an unencrypted key there.
  - The terminal no longer reads `~/.ssh/config` directly, so exotic directives such as `ProxyCommand` no longer apply (the parsed HostName/User/Port/IdentityFile/ProxyJump still do). This makes the terminal consistent with the rest of the app.

### Security
- Validate public-key format and reject control characters before embedding keys in remote shell commands.
- Sanitize `sshd_config` directive edits and scope the disabled `Include` to the cloud-init drop-in only.
- Store `config.toml`, `hosts.toml`, and `snippets.toml` with `0600` permissions, and remove temp files left by a failed save.
- Reject `..` traversal and null bytes in SFTP upload/download paths.
- Pin all GitHub Actions to commit SHAs and verify release archives against `SHA256SUMS` in `install.sh` (with a `shasum` fallback on macOS).

### Bug Fixes
- **Log files no longer fill the disk**: On startup OmnySSH now prunes rolling log files older than 7 days from its config directory. The cleanup is best-effort and never blocks startup, works on every platform via the native config path, and only touches `omnyssh.log*` files — `config.toml`, `hosts.toml`, and `snippets.toml` are left untouched.
- **Man page now installs reliably**: `install.sh` failed to install the man page into the system directory (e.g. `/usr/local/share/man` on macOS) because it never elevated with `sudo`, so `man omny` reported "No manual entry". The man page is now downloaded to a temp file first and installed with a `sudo` fallback, mirroring the binary install.
- **Remote command exit status is no longer lost** when it arrives after EOF.
- **Multi-file deletes fixed**: all in-flight remote deletes are tracked before the panel refreshes, and every error from a multi-file local delete is reported.
- **Terminal is always restored on exit**, even if an earlier teardown step fails.
- **Docker view scales to many containers**: `docker inspect` is batched to stay under `ARG_MAX`.

### Documentation
- Fixed inaccurate docs: `--verbose` now correctly states logs are written to a log file (not stderr) in `--help`, the man page, and the README; `install.sh` prints the OS-specific config directory; corrected the horizontal split keybinding in the changelog (`Ctrl+]`, not `Ctrl+-`).
- More inaccurate docs corrected: `--theme` is now documented as persisting your choice to `config.toml` (it always wrote the config back, but the README called it temporary); the `[ui]` config example no longer lists `show_ip`, `show_uptime`, `card_layout`, and `border_style`, which are parsed for forward compatibility but not yet wired to the renderer; and the "static binary" wording was corrected since the x86_64 Linux and macOS builds are not statically linked.

### Other
- Removed the vestigial empty `package.json` and `package-lock.json` (left over from an earlier project name); they served no purpose in this Rust project.
- Removed the unused `config/default.toml` template — it was never read by the app, and the README already carries the canonical config example.
- CI now fails if the committed `doc/omny.1` man page drifts from `src/cli.rs`, so the generated man page can no longer go stale.
- Removed dead code with no effect on runtime behaviour: unused functions with no callers (`Screen::title`, `Host::id`, `key_path_for_host`, the superseded `host_list` render path, and two unused popup renderers), plus three methods that were only reachable from their own tests (`FileManagerPanel::clamp_scroll`, `ProbeOutput::section_names`, `KeySetupMachine::password_disabled`) and those tests.
- Made the `sshd` rollback backup lookup portable so it also works on BusyBox/Alpine hosts.

---

## 1.0.3 — 2026-05-21

### Features
- **Termux / Android support**: Releases now include a statically linked `aarch64-unknown-linux-musl` build that runs on Termux. `install.sh` detects Termux via `$TERMUX_VERSION` / `$PREFIX` and installs the binary into `$PREFIX/bin` (and the man page into `$PREFIX/share/man/man1`) without `sudo`.

---

## 1.0.2 — 2026-05-16

### Features
- **Automatic update checks**: On startup OmnySSH checks GitHub Releases for a newer version and shows a popup when one is available. You can install the update, skip that version, or disable checks entirely. Failed or offline checks are silent and never delay startup.
- **In-app self-update**: For manual / `install.sh` installs on Linux and macOS, an update can be downloaded and installed from within the app — the release archive is verified against its SHA-256 checksum before the binary is replaced. Homebrew, Cargo, and Nix installs instead show the matching upgrade command.
- **Top processes on the detail page**: The server detail view now shows the three busiest processes by CPU usage, along with their CPU and memory percentages.

### Bug Fixes
- **Windows double input fixed**: Each keystroke is now registered once instead of twice (e.g. "j" no longer produced "jj"). Key-release events reported by the Windows console are no longer treated as input.
- **Ubuntu 22.04 compatibility**: Linux release binaries are now built against an older glibc, fixing the `version 'GLIBC_2.39' not found` error when running on Ubuntu 22.04 and similarly aged distributions.
- **Mouse scroll inside full-screen apps fixed**: On the Terminal screen, the mouse wheel now scrolls inside `vim`, `less`, `htop`, and other alternate-screen apps. The wheel is forwarded to the foreground application — as native mouse-wheel events when it enabled mouse reporting, or as cursor-key presses otherwise. The normal screen still scrolls local scrollback.
- **Multi-line paste into the terminal fixed**: Bracketed paste is now implemented. Pasting multi-line text into the Terminal screen no longer drops the first characters, and editors like `vim` insert it verbatim without cascading auto-indent.
- **Top processes exclude OmnySSH's own connection**: The detail-page top-processes panel no longer lists OmnySSH's metric-polling SSH connection. Its `sshd` process chain is filtered out by PID, so an idle server shows real workload while SSH sessions from other users still appear.
- **Bracketed paste restored after system SSH**: Connecting to a host via the system `ssh` binary no longer leaves bracketed paste disabled on return, so multi-line paste into the terminal keeps working.
- **Host keys are now verified**: A server's host key is recorded in `~/.ssh/known_hosts` on first connection (trust on first use). A later key change — or an unreadable `known_hosts` — refuses the connection instead of silently accepting an unverified key.
- **Password authentication reliably disabled**: Auto SSH Key Setup now adds the `PasswordAuthentication`, `UsePAM`, and challenge/keyboard-interactive directives when `sshd_config` omits them, so password login is disabled even on configs that previously relied on compiled-in defaults.
- **System SSH launch failure no longer quits the app**: If the `ssh` binary cannot be started, OmnySSH restores the TUI and reports the error in the status bar instead of exiting.
- **Remote command exit status honoured**: SSH command execution now exposes a non-zero remote exit status; the key-setup sudo check uses it to detect missing sudo access correctly.

### Other
- Release archives are now published alongside a `SHA256SUMS` checksum file.
- **Internal refactor**: The oversized `app.rs` was split into focused submodules under `src/app/` (host, snippets, file manager, terminal, update, input, and action dispatch). This is a pure code reorganization with no change in behaviour.
- **Test coverage**: Added unit tests for previously untested core logic — snippet parameter substitution, host/snippet form validation, the SSH-config/manual host merge, `hosts.toml` serialization, form-field UTF-8 editing, host/snippet filtering and sorting, file-panel selection, and terminal pane state.

---

## 1.0.1 — 2026-04-22

### Bug Fixes
- **TUI display corruption fixed**: Log output no longer bleeds through the TUI interface. All logging is now redirected to a log file (`~/.config/omnyssh/omnyssh.log` on Linux, `~/Library/Application Support/omnyssh/omnyssh.log.*` on macOS) instead of stderr, preventing raw error messages (such as SSH timeout warnings) from corrupting the terminal display during background operations.
- **Error notifications**: Connection failures, discovery timeouts, and snippet execution errors are now displayed as concise notifications in the status bar instead of being silently logged.
- **SFTP connection freeze fixed**: SFTP connections now run in the background with a 30-second timeout, preventing the UI from freezing indefinitely when connecting to slow or unresponsive servers. A "Connecting… (30s timeout)" indicator is displayed during the connection attempt.
- **Terminal scroll fixed**: Two-finger trackpad and mouse-wheel scroll on the Terminal screen now scrolls local scrollback instead of cycling the remote shell's command history. Previously, mouse capture was disabled on the Terminal screen to allow native mouse text selection, which caused host terminal emulators to translate scroll gestures into ArrowUp/ArrowDown keys that bash readline interpreted as history navigation. Mouse capture is now kept on across all screens.
- **Native drag-to-select preserved**: Mouse capture now enables only button and scroll-wheel reporting (`?1000h` + `?1006h`), dropping the aggressive any-motion tracking (`?1002h` / `?1003h`) that crossterm enables by default. In terminals that honor the modifier-bypass for mouse reporting (iTerm2 on macOS, most Linux terminals), hold `Option` (iTerm2) or `Shift` (Linux) while dragging to select and copy text in the Terminal screen without the application intercepting the drag. Note: macOS Terminal.app does not support modifier-bypass for mouse reporting at all — users on Terminal.app should switch to iTerm2 or a similar emulator for in-app text selection.

---

## 1.0.0 — 2026-04-18

First production-ready release of OmnySSH.

### Features

#### Dashboard
- Server cards with live **CPU / RAM / Disk** metrics, uptime, and load average
- Colour-coded thresholds: 🟢 < 60%, 🟡 60–85%, 🔴 > 85%
- Async metrics collection — each host polled independently via SSH
- Cross-platform parsers: Linux (`top`/`free`/`/proc/stat`), macOS (`vm_stat`), Alpine BusyBox
- Configurable poll interval (default 30 s) with exponential backoff on failure
- Sort by name / CPU / RAM / status (`s`)
- Filter by tag (`t`)
- Manual refresh (`r`)
- Connection status indicator: `●` online, `◐` connecting, `✗` failed
- Connection pool: one SSH connection per host, reused for all metrics

#### Host management
- Host list with instant fuzzy search (`/`)
- Automatic import from `~/.ssh/config` (Host, HostName, User, Port, IdentityFile, ProxyJump, Include)
- Add / Edit / Delete hosts via TUI forms
- Tags and notes for each host
- Persistence in `~/.config/omnyssh/hosts.toml` — original `~/.ssh/config` is never modified
- Delete confirmation popup

#### File Manager (SFTP)
- Split-panel browser: local files ↔ remote SFTP
- Directory navigation with `h/j/k/l` and arrow keys
- File operations: upload, download, delete, mkdir, rename
- Progress bar with percentage for transfers
- Multiple file selection with `Space`
- Copy (`c`) / Paste (`p`) across panels
- Plain-text file preview
- Host-picker popup for remote panel

#### Snippets
- Save, edit, and delete global and host-scoped command snippets
- Parameterised snippets with `{{placeholder}}` syntax
- Quick-execute (`x`): run ad-hoc commands from the Dashboard
- Broadcast mode (`b`): execute on multiple hosts in parallel
- Fuzzy search on the Snippets screen
- Persistence in `~/.config/omnyssh/snippets.toml`

#### Multi-session terminal
- PTY-backed terminal with tabs (`Ctrl+T` / `Ctrl+W`)
- Split-view: `Ctrl+\` vertical, `Ctrl+]` horizontal
- Tab navigation with `Ctrl+Right` / `Ctrl+Left`
- Activity indicator on tabs with unseen output
- Full VT100 terminal emulation (`portable-pty` + `vt100`)
- Non-blocking render — terminal never freezes the UI

#### Themes & Configuration
- 4 built-in colour themes: `default`, `dracula`, `nord`, `gruvbox`
- `--theme <THEME>` CLI flag to override theme at runtime: `omny --theme dracula`
- Fully configurable keybindings via `[keybindings]` in config
- `--config <FILE>` flag to load a custom config
- `--help` / `--version` flags

#### General
- Cross-platform: Linux, macOS, Windows (single static binary)
- Panic hook that restores the terminal before printing backtrace
- `russh`-based async SSH client (no external `ssh` binary dependency for metrics)
- CI: GitHub Actions matrix for Ubuntu, macOS, Windows

---

## Development history

| Date | Version | Milestone |
|------|---------|-----------|
| 2026-04-04 | `0.0.1` | Project skeleton — TUI shell, event loop, placeholder screens |
| 2026-04-05 | `0.1.0` | Host list, SSH connect, fuzzy search — first MVP |
| 2026-04-06 | `0.2.0` | Live metrics dashboard with async polling |
| 2026-04-07 | `0.3.0` | Command snippets, quick-execute, broadcast |
| 2026-04-08 | `0.4.0` | SFTP file manager with split-panel UI |
| 2026-04-09 | `0.5.0` | Multi-session PTY tabs and split-view |
| 2026-04-10 | **`1.0.0`** | **Themes, configurable keybindings, production release** |
