#!/bin/bash

# Deploy script for easyp HTTPS server
# Usage: ./deploy.sh <target_host>

set -e  # Exit on any error

STAGING=--staging
STAGING=
PROFILE=lto
		  
KEEPALIVE=y

if [ "$1" = quitafter ]
then
	KEEPALIVE=
	shift
fi

if [ -z "$1" ]
then
	SRV=$(cat .remote)
else
	SRV="$1"
fi

echo "DEBUG: Target server is $SRV"
echo "DEBUG: Starting deployment process..."

source ~/.cargo/env

# Build if needed
[ -f target/debug/easyp ] || RUSTC_WRAPPER= cargo build --bin easyp
if [ -z "$(find src/ */src/ -type f -newer target/debug/easyp 2>/dev/null)" ] || RUSTC_WRAPPER= cargo build --bin easyp --profile $PROFILE
then
	echo "DEBUG: Building completed, starting deployment..."
	
	echo "DEBUG: Killing existing easyp processes on remote server..."
	ssh root@$SRV "pkill easyp;sleep 1;pkill -9 easyp; true" && echo "DEBUG: Process cleanup completed"
	
	echo "DEBUG: Ensuring certificate directories are properly separated..."
	ssh root@$SRV "mkdir -p /var/lib/easyp/certs/staging /var/lib/easyp/certs/production" && echo "DEBUG: Certificate directories prepared"
	
	echo "DEBUG: Syncing binary to remote server..."
	rsync -avz ../target/$PROFILE/easyp root@$SRV: && echo "DEBUG: Binary sync completed"
	
	echo "DEBUG: Starting server in background..."
	echo "DEBUG: Using staging flag: [$STAGING]"
	ssh root@$SRV "pkill easyp; chmod +x easyp; nohup ./easyp --root /var/www/html $STAGING $VERBOSE $BOGUS > server.log 2>&1 &"
	echo "DEBUG: Server startup command sent to remote server"
	
	echo "DEBUG: Waiting 10 seconds for server to initialize..."
	sleep 10
	
	echo "DEBUG: Checking if server process is running on remote server..."
	if ssh root@$SRV "pgrep easyp > /dev/null"; then
		echo "DEBUG: Server process is running on remote server"
		echo "DEBUG: Checking server logs for startup completion..."
		ssh root@$SRV "tail -5 server.log"
	else
		echo "DEBUG: ERROR - Server process not found on remote server!"
		echo "DEBUG: Checking server logs..."
		ssh root@$SRV "tail -20 server.log" || echo "DEBUG: No server log found"
		exit 1
	fi
	
	echo "DEBUG: Testing server connectivity..."
	echo "DEBUG: Checking if port 80 is open..."
	if timeout 5 bash -c "echo > /dev/tcp/$SRV/80" 2>/dev/null; then
		echo "DEBUG: Port 80 is open"
	else
		echo "DEBUG: WARNING - Port 80 is not accessible"
	fi
	
	echo "DEBUG: Checking if port 443 is open..."
	if timeout 5 bash -c "echo > /dev/tcp/$SRV/443" 2>/dev/null; then
		echo "DEBUG: Port 443 is open"
	else
		echo "DEBUG: WARNING - Port 443 is not accessible"
	fi
	
	echo "DEBUG: Starting HTTP test with 10 second timeout..."
	echo === HTTP TEST ===
	if timeout 10 curl -v --connect-timeout 5 --max-time 10 "http://$SRV"; then
		echo "DEBUG: HTTP test completed successfully"
	else
		echo "ERROR: HTTP test failed or timed out"
		exit 1
	fi
	
	sleep 1
	
	echo "DEBUG: Starting HTTPS test with 15 second timeout..."
	echo === HTTPS TEST ===
	if timeout 15 curl -v --connect-timeout 10 --max-time 15 \
		--tlsv1.2 --tlsv1.3 \
		--ciphers 'ECDHE+AESGCM:ECDHE+CHACHA20:DHE+AESGCM:DHE+CHACHA20:!aNULL:!MD5:!DSS' \
		--retry 2 --retry-delay 1 \
		-k "https://$SRV"; then
		echo "DEBUG: HTTPS test completed successfully"
	else
		echo "ERROR: HTTPS test failed or timed out"
		echo "DEBUG: Attempting HTTPS test with different SSL options..."
		if timeout 15 curl -v --connect-timeout 10 --max-time 15 \
			--tls-max 1.3 --tlsv1.2 \
			--insecure --retry 1 \
			"https://$SRV" > /tmp/curl_output.log 2>&1; then
			echo "DEBUG: HTTPS test with fallback options completed"
		else
			echo "ERROR: HTTPS test failed with all SSL options"
			echo "DEBUG: Curl output:"
			cat /tmp/curl_output.log
			exit 1
		fi
	fi
	
	sleep 1
	
	echo "DEBUG: Testing certificate stability..."
	echo === CERTIFICATE STABILITY TEST ===
	CERT1=$(timeout 15 openssl s_client -connect $SRV:443 -servername $SRV < /dev/null 2>/dev/null | openssl x509 -fingerprint -sha256 -noout 2>/dev/null | cut -d= -f2)
	sleep 3
	CERT2=$(timeout 15 openssl s_client -connect $SRV:443 -servername $SRV < /dev/null 2>/dev/null | openssl x509 -fingerprint -sha256 -noout 2>/dev/null | cut -d= -f2)
	
	if [ -z "$CERT1" ] || [ -z "$CERT2" ]; then
		echo "ERROR: Certificate fingerprint test failed - could not retrieve certificates"
		exit 1
	fi
	
	if [ "$CERT1" != "$CERT2" ]; then
		echo "ERROR: Certificate stability test failed - certificate changed between requests"
		echo "   First cert:  $CERT1"
		echo "   Second cert: $CERT2"
		exit 1
	else
		echo "DEBUG: Certificate stability test passed - same certificate on multiple requests"
	fi
	
	echo "DEBUG: Testing wget with security disabled..."
	echo === WGET TEST ===
	if ! timeout 20 wget --no-check-certificate --timeout=15 --tries=1 -q -O /tmp/wget_remote_test.html "https://$SRV" 2>/dev/null; then
		echo "ERROR: wget test failed"
		exit 1
	fi
	
	# Check if wget got non-empty content
	if [ ! -s /tmp/wget_remote_test.html ]; then
		echo "ERROR: wget test failed - received empty response"
		exit 1
	fi
	
	# Check if content looks like HTML
	if ! grep -q "<html\|<!DOCTYPE" /tmp/wget_remote_test.html 2>/dev/null; then
		echo "ERROR: wget test failed - response doesn't appear to be HTML"
		echo "   Response content:"
		head -5 /tmp/wget_remote_test.html
		exit 1
	fi
	
	echo "DEBUG: wget test passed - received valid HTML content"
	rm -f /tmp/wget_remote_test.html
	
	echo === END TESTS ===
	
	if [ -z "$KEEPALIVE" ]
	then
		ssh root@$SRV "pkill easyp; sleep 1; pkill -9 easyp" || echo "DEBUG: Server process cleanup completed"
		echo "DEBUG: Stopping server process on remote server..."
	fi
	
	echo "DEBUG: Test script completed successfully"
	exit 0
fi
