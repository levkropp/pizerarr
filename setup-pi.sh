#!/bin/bash
# Run this on the Pi after scp'ing the pizerarr binary to /tmp/pizerarr

set -e

# Make filesystem writable (PiKVM is read-only by default)
rw

# Install binary
mv /tmp/pizerarr /usr/local/bin/pizerarr
chmod +x /usr/local/bin/pizerarr

# Create directories
mkdir -p /mnt/media /mnt/downloads

# Stop PiKVM services to free RAM
systemctl disable --now kvmd kvmd-otg kvmd-nginx kvmd-vnc kvmd-ipmi 2>/dev/null || true

# Create systemd service
cat > /etc/systemd/system/pizerarr.service << 'EOF'
[Unit]
Description=pizerarr media server
After=network-online.target

[Service]
ExecStart=/usr/local/bin/pizerarr
Environment=PIZERARR_MEDIA_DIR=/mnt/media
Environment=PIZERARR_DOWNLOAD_DIR=/mnt/downloads
Environment=PIZERARR_PORT=8080
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now pizerarr

echo ""
echo "pizerarr is running at http://$(hostname -I | awk '{print $1}'):8080"
