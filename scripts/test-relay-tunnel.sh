#!/bin/bash
# Subspace Relay end to end on Linux, without root: a "phone" and a
# "computer" in private network namespaces joined by a veth, a pretend
# website behind the phone, and `slipstream relay connect` on the computer.
# Checks a download and UDP go through the tunnel, and that routes and the
# adapter are gone afterwards.
#
#   scripts/test-relay-tunnel.sh [path/to/slipstream]   (default target/debug)
if [ "${1:-}" != "--inside" ]; then
    BIN="$(realpath "${1:-target/debug/slipstream}")"
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
    timeout 90 unshare -rnm --propagation private bash "$0" --inside "$BIN" "$WORK" > "$WORK/out.txt" 2>&1
    sed 's/\x1b\[[0-9;]*m//g' "$WORK/out.txt"
    grep -q '^PASS$' "$WORK/out.txt"
    exit $?
fi
# Runs as "root" inside a private user+network+mount namespace: the "phone".
BIN="$2"; WORK="$3"
cd "$WORK"
ip link set lo up
ip link add dummy0 type dummy && ip addr add 203.0.113.10/32 dev dummy0 && ip addr add 203.0.113.53/32 dev dummy0 && ip link set dummy0 up
head -c 5000000 /dev/urandom > site.bin
python3 -I -m http.server --bind 203.0.113.10 8080 >/dev/null 2>&1 &
python3 -I -c '
import socket
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.bind(("203.0.113.10",5353))
while True:
    d,a=s.recvfrom(2048); s.sendto(d[::-1],a)' >/dev/null 2>&1 &
# A stand-in DNS server: the computer's lookups go to the tunnel's gateway,
# which the tunnel sends here (--dns).
python3 -I -c '
import socket
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.bind(("203.0.113.53",53))
while True:
    d,a=s.recvfrom(2048); s.sendto(b"dns:"+d,a)' >/dev/null 2>&1 &
# The "computer": its own network namespace, joined to the phone by a veth.
unshare -n sleep 600 >/dev/null 2>&1 & CPID=$!
sleep 0.3
ip link add veth-p type veth peer name veth-c netns $CPID
ip addr add 192.168.49.1/24 dev veth-p && ip link set veth-p up
nsenter -t $CPID -n sh -c 'ip link set lo up; ip addr add 192.168.49.2/24 dev veth-c; ip link set veth-c up'
"$BIN" relay serve --ip 192.168.49.1 > relay-serve.log 2>&1 & RPID=$!
sleep 1
echo "== before the tunnel (should fail: no route)"
nsenter -t $CPID -n curl -s -m 3 -o /dev/null -w '%{http_code}\n' http://203.0.113.10:8080/site.bin || echo "unreachable, as expected"
echo "== tunnel up"
nsenter -t $CPID -n env SLIPSTREAM_RELAY_NO_RESOLVED=1 RUST_LOG=info "$BIN" relay connect --dns 203.0.113.53 > relay-connect.log 2>&1 & TPID=$!
sleep 3
nsenter -t $CPID -n ip route
echo "== download through the relay"
nsenter -t $CPID -n curl -s -m 30 -o got.bin -w 'http %{http_code}, %{size_download} bytes, %{speed_download} B/s\n' http://203.0.113.10:8080/site.bin
cmp -s site.bin got.bin && echo "download matches" && OK_TCP=1 || echo "DOWNLOAD MISMATCH"
echo "== UDP through the relay"
nsenter -t $CPID -n python3 -I -c '
import socket
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(5)
s.sendto(b"subspace",("203.0.113.10",5353)); print("udp reply:", s.recv(100))' > udp.txt
cat udp.txt
grep -q ecapsbus udp.txt && OK_UDP=1
echo "== DNS through the relay (sent to the tunnel's gateway)"
nsenter -t $CPID -n python3 -I -c '
import socket
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.settimeout(5)
s.sendto(b"query",("10.233.49.1",53)); print("dns reply:", s.recv(100))' > dns.txt
cat dns.txt
grep -q "dns:query" dns.txt && OK_DNS=1
echo "== tunnel down"
kill -INT $TPID; wait $TPID; echo "connect exit $?"
nsenter -t $CPID -n ip route
nsenter -t $CPID -n ip route | grep -q slipstream0 && echo "ROUTES LEFT BEHIND" || OK_ROUTES=1
nsenter -t $CPID -n ip link show slipstream0 >/dev/null 2>&1 && echo "ADAPTER LEFT BEHIND" || OK_LINK=1
cat /etc/resolv.conf | head -3
echo "== the app's helper mode (--control)"
cat > control.py <<'PY'
import json, socket, sys, time
srv = socket.socket(); srv.bind(("127.0.0.1", 47199)); srv.listen(1); srv.settimeout(20)
c, _ = srv.accept(); f = c.makefile("r")
assert f.readline().strip() == "test-token", "token"
ups = 0
while ups < 3:
    st = json.loads(f.readline())
    if "Up" in st: ups += 1
    elif st != "Linking": sys.exit("unexpected %r" % st)
print("helper up, status lines received")
c.shutdown(socket.SHUT_WR)  # the app pressing Disconnect
for line in f:
    st = json.loads(line)
    if "Down" in st: print("helper down:", st["Down"]); sys.exit(0)
sys.exit("no Down status")
PY
nsenter -t $CPID -n python3 -I control.py > control.txt 2>&1 & KPID=$!
sleep 0.5
nsenter -t $CPID -n env SLIPSTREAM_RELAY_NO_RESOLVED=1 "$BIN" relay connect --control 127.0.0.1:47199 --token test-token > helper.log 2>&1 & HPID=$!
wait $KPID && OK_HELPER=1; cat control.txt
wait $HPID; echo "helper exit $?"
nsenter -t $CPID -n ip link show slipstream0 >/dev/null 2>&1 && { echo "ADAPTER LEFT BEHIND"; OK_HELPER=; }
kill $(jobs -p) 2>/dev/null; wait 2>/dev/null
echo "== relay log"; tail -5 relay-serve.log; echo "== connect log"; tail -8 relay-connect.log
echo "checks: tcp=${OK_TCP:-0} udp=${OK_UDP:-0} routes=${OK_ROUTES:-0} adapter=${OK_LINK:-0} helper=${OK_HELPER:-0} dns=${OK_DNS:-0}"
[ "${OK_TCP:-}${OK_UDP:-}${OK_ROUTES:-}${OK_LINK:-}${OK_HELPER:-}${OK_DNS:-}" = 111111 ] && echo PASS || echo FAIL
