#!/bin/sh
# NanoKVM (Linux, BusyBox init): /etc/init.d/S99gh-updater  (chmod +x)
# NanoKVM の init 方式は未確認。/etc/init.d/S* が使える場合の例。
case "$1" in
  start)  start-stop-daemon -S -b -m -p /var/run/gh-updater.pid -x /data/gh-updater/gh-updater -- --config /data/gh-updater/updater.nanokvm.toml ;;
  stop)   start-stop-daemon -K -p /var/run/gh-updater.pid ;;
  restart) "$0" stop; "$0" start ;;
esac
