#!/bin/bash
# Measure latency to Polymarket CLOB API from different AWS regions
# This helps identify the optimal region for deployment

set -e

POLYMARKET_API="https://clob.polymarket.com"
REGIONS=("us-east-1" "us-east-2" "us-west-1" "us-west-2" "eu-west-1")

echo "=============================================="
echo "Polymarket CLOB API Latency Test"
echo "=============================================="
echo ""
echo "Testing from local machine first..."
echo ""

# Test from local machine
echo "Local machine latency:"
for i in {1..5}; do
    curl -w "  Request $i: %{time_total}s (connect: %{time_connect}s, DNS: %{time_namelookup}s)\n" \
         -o /dev/null -s "$POLYMARKET_API/health"
done

echo ""
echo "----------------------------------------------"
echo "DNS lookup for clob.polymarket.com:"
dig +short clob.polymarket.com | head -5
echo ""

# Check if we can determine the hosting region
echo "----------------------------------------------"
echo "Checking IP geolocation..."
IP=$(dig +short clob.polymarket.com | head -1)
if [ -n "$IP" ]; then
    # Try to get ASN info
    whois "$IP" 2>/dev/null | grep -i -E "(netname|orgname|country)" | head -5 || echo "Could not determine hosting info"
fi

echo ""
echo "=============================================="
echo "Recommendation"
echo "=============================================="
echo ""
echo "Based on typical Polymarket infrastructure:"
echo "  - Primary region: us-east-1 (Virginia)"
echo "  - Expected latency from us-east-1: 5-15ms"
echo "  - Expected latency from other regions: 30-100ms+"
echo ""
echo "To test from AWS regions, launch t3.micro instances and run:"
echo "  curl -w '%{time_total}s\n' -o /dev/null -s $POLYMARKET_API/health"
echo ""
