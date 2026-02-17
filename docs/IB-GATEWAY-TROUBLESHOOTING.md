# IB Gateway Docker Troubleshooting Guide

## Quick Diagnosis

### 1. Check Container Status
```bash
docker ps | grep ib-gateway
docker inspect --format='{{.State.Health.Status}}' ib-gateway
```

Expected: Container running with `healthy` status after 60s initialization

### 2. Check Gateway Logs
```bash
docker logs ib-gateway --tail 50
```

Look for:
- ✅ "Configuration tasks completed" - Gateway initialized
- ✅ "Connecting to server" - Login attempted
- ⚠️  "Two-Factor Authentication required" - Needs mobile approval
- ❌ "Login failed" - Wrong credentials or account locked

### 3. Check 2FA Status
- **Most common issue**: Gateway is waiting for 2FA approval
- Check IBKR Mobile app for push notification
- You MUST approve within timeout window (default 3 minutes)
- After approval, session lasts ~7 days

### 4. Test TCP Connection
```bash
# From EC2 host
timeout 1 bash -c '</dev/tcp/127.0.0.1/4002'
echo $?  # Should be 0 if port is open

# From inside container
docker exec ib-gateway telnet localhost 4002
```

Expected: Connection opens (proves socat relay works)

### 5. Test API Handshake
```python
from ib_async import IB
import asyncio

async def test():
    ib = IB()
    try:
        await ib.connectAsync('127.0.0.1', 4002, clientId=999, timeout=10)
        print("✅ Connected successfully!")
        # Wait for nextValidId
        await asyncio.sleep(2)
        print(f"Valid order ID: {ib.client.getReqId()}")
        ib.disconnect()
    except Exception as e:
        print(f"❌ Connection failed: {e}")

asyncio.run(test())
```

## Common Failure Scenarios

### Scenario 1: Immediate Disconnect (API Handshake Fails)

**Symptoms:**
- `TimeoutError` during connect
- Logs show "Connected" then immediately "Disconnected"
- TCP connection succeeds but API handshake never completes

**Root Causes:**
1. **2FA not approved** - Gateway waiting for mobile push notification
2. **API access disabled** - IBKR account settings block API
3. **Wrong account type** - Paper trading credentials on live port (or vice versa)
4. **Competing session** - Another TWS/Gateway instance using same username
5. **ib_insync bug #303** - First connect always fails on fresh starts

**Fix:**
1. Check IBKR Mobile app - approve 2FA notification
2. Verify IBKR account settings:
   - Login to Account Management → Settings → API
   - Enable "ActiveX and Socket Clients"
   - Set socket port to 4002 (paper) or 4001 (live)
3. Use correct port: 4002 for paper, 4001 for live
4. Set `EXISTING_SESSION_DETECTED_ACTION=primaryoverride` in Docker env
5. Implement retry logic (our code now has this built-in)

### Scenario 2: Connection Timeout (No Response)

**Symptoms:**
- `ConnectionRefusedError` or `OSError`
- Can't establish TCP connection at all

**Root Causes:**
1. **Gateway still initializing** - Takes 30-60s after container start
2. **Container crashed** - Check `docker ps`
3. **Port binding issue** - Ports not exposed correctly
4. **Firewall** - Security group blocking ports (shouldn't affect localhost)

**Fix:**
1. Wait 60s after `docker run` before connecting
2. Use health checks: `--health-cmd` in docker run
3. Verify ports: `docker port ib-gateway`
4. Check ulimits: `--ulimit nofile=10000:10000`

### Scenario 3: Container Unhealthy

**Symptoms:**
- `docker inspect` shows `unhealthy` status
- Health check keeps failing

**Root Causes:**
1. **socat not started** - Internal TCP relay failed
2. **Java heap exhaustion** - Gateway out of memory
3. **File descriptor limit** - ulimit too low

**Fix:**
1. Check container logs for socat errors
2. Increase memory: `-e JAVA_HEAP_SIZE=1024`
3. Set ulimits: `--ulimit nofile=10000:10000`
4. Restart container: `docker restart ib-gateway`

### Scenario 4: Weekly Re-Authentication Failures

**Symptoms:**
- Works fine for days, then suddenly fails
- Happens around Sunday 1 AM ET

**Root Cause:**
- IBKR invalidates sessions weekly (Sunday ~1:00 AM ET)
- Requires fresh 2FA approval

**Fix:**
1. Set `TWOFA_TIMEOUT_ACTION=restart` (container auto-retries)
2. Set `RELOGIN_AFTER_TWOFA_TIMEOUT=yes` (keeps retrying)
3. Approve 2FA notification when it arrives (~once per week)
4. Alternative: Extract TOTP secret for fully automated 2FA (see below)

## Advanced: Fully Automated 2FA

For production systems that can't have weekly manual intervention:

### Option 1: TOTP Secret Key (Retail Accounts)

1. **Enroll in IBKR Mobile Authenticator** (TOTP-based 2FA)
2. **Use 2FAS or similar app** that allows exporting the secret key
3. **Extract the TOTP secret** during enrollment
4. **Add to Docker environment:**
   ```bash
   -e TOTP_KEY=your-base32-secret-key-here
   ```
5. **Install oathtool in container** (heshiming/ibga project provides this)

This generates 6-digit codes programmatically, eliminating human intervention.

### Option 2: OAuth (Institutional Only)

- Available to Financial Advisors, Introducing Brokers, approved vendors
- NOT available to individual retail traders
- Uses OAuth 1.0a or newer OAuth 2.0 (via IBind library)
- Bypasses Gateway entirely - direct REST API access

## VNC Debugging

Our deployment enables VNC for visual debugging:

```bash
# SSH tunnel from your local machine
ssh -L 5900:127.0.0.1:5900 ubuntu@<EC2_IP>

# Open VNC viewer
vncviewer localhost:5900
# Password: ibgateway
```

You can see the actual Gateway UI, login prompts, and 2FA dialogs.

## Production Checklist

- [ ] Health checks configured (60s start period)
- [ ] Retry logic in API client (5 attempts, 5s delay)
- [ ] 2FA strategy chosen (push notification vs TOTP)
- [ ] Auto-restart configured (`AUTO_RESTART_TIME=02:00 AM`)
- [ ] Ports bound to localhost only (`127.0.0.1:4002`)
- [ ] ulimits set (`--ulimit nofile=10000:10000`)
- [ ] VNC enabled for debugging (disable in prod if not needed)
- [ ] Monitoring alerts for connection failures

## Key Configuration Reference

| Setting | Value | Purpose |
|---------|-------|---------|
| `TRADING_MODE` | `paper` or `live` | Account type |
| `TWOFA_TIMEOUT_ACTION` | `restart` | Auto-retry on 2FA timeout |
| `RELOGIN_AFTER_TWOFA_TIMEOUT` | `yes` | Keep retrying login |
| `AUTO_RESTART_TIME` | `02:00 AM` | Daily restart (after IBKR maintenance) |
| `EXISTING_SESSION_DETECTED_ACTION` | `primaryoverride` | Take control from other sessions |
| `READ_ONLY_API` | `no` | Enable trading |
| `VNC_SERVER_PASSWORD` | Set for debugging | VNC access password |
| Port 4001 | Live trading | TWS API port |
| Port 4002 | Paper trading | TWS API port |
| Port 5900 | VNC | Visual debugging |

## Resources

- **gnzsnz/ib-gateway-docker**: https://github.com/gnzsnz/ib-gateway-docker
- **IBC (IB Controller)**: https://github.com/IbcAlpha/IBC
- **ib_async library**: https://github.com/erdewit/ib_async
- **ib_insync bug #303**: https://github.com/erdewit/ib_insync/issues/303
- **IBKR API Docs**: https://interactivebrokers.github.io/tws-api/
- **heshiming/ibga (TOTP automation)**: https://github.com/heshiming/ibga
- **Voyz/IBind (OAuth)**: https://github.com/Voyz/ibind

## Emergency Recovery

If completely stuck:

1. **Nuke and rebuild:**
   ```bash
   docker rm -f ib-gateway
   ./scripts/deploy-ib-options.sh start-gateway
   ```

2. **Check IBKR account status:**
   - Login to Account Management
   - Check for locked account, API restrictions, unpaid fees

3. **Try standalone TWS:**
   - Download TWS desktop app
   - Confirm you can login and trade manually
   - If TWS works but Gateway doesn't, it's a Docker config issue

4. **Contact IBKR:**
   - If nothing works, account might have API restrictions
   - IBKR support can confirm API access is enabled
