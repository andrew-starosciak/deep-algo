"""Programmatic contract selection using real IB option chain data.

The LLM provides direction + expected move + timeline.
This module queries IB for actual strikes/expirations and picks
the best contract with real bid/ask data.
"""

from __future__ import annotations

import datetime as _dt
import logging
from dataclasses import dataclass
from decimal import Decimal

from schemas.thesis import ContractSpec, Thesis

logger = logging.getLogger(__name__)


@dataclass
class SelectorConfig:
    """Tuning knobs for contract selection."""

    # Expiry: at least this multiple of catalyst_timeline_days
    expiry_multiple: float = 2.0
    # Absolute minimum DTE to avoid steep theta decay
    min_dte: int = 30
    # Target OTM%: fraction of expected_move_pct (e.g. 0.5 = half the expected move)
    otm_fraction: float = 0.5
    # Maximum OTM% to allow
    max_otm_pct: float = 20.0
    # Minimum option mid price to accept
    min_mid_price: Decimal = Decimal("0.10")
    # Maximum bid-ask spread as fraction of mid
    max_spread_pct: float = 0.30


class ContractSelector:
    """Select an options contract from real IB data based on a thesis."""

    def __init__(self, ib_client, config: SelectorConfig | None = None):
        self.ib = ib_client
        self.config = config or SelectorConfig()

    async def select(self, thesis: Thesis) -> ContractSpec | None:
        """Pick the best contract for a thesis using live IB data.

        Returns ContractSpec with real bid/ask prices, or None if no
        suitable contract is found.
        """
        ticker = thesis.ticker

        # 1. Get current stock price
        try:
            stock_price = await self.ib.get_stock_price(ticker)
        except Exception as e:
            logger.warning("Could not get stock price for %s: %s", ticker, e)
            return None
        logger.info("Stock price for %s: $%s", ticker, stock_price)

        # 2. Get option chain (expirations + strikes)
        try:
            chain = await self.ib.get_option_chain(ticker)
        except Exception as e:
            logger.warning("Could not get option chain for %s: %s", ticker, e)
            return None

        expirations = chain["expirations"]
        strikes = chain["strikes"]

        if not expirations or not strikes:
            logger.warning("Empty option chain for %s", ticker)
            return None

        # 3. Pick expiry: first available >= expiry_multiple * catalyst_timeline_days
        min_dte = max(
            self.config.min_dte,
            int(thesis.catalyst_timeline_days * self.config.expiry_multiple),
        )
        target_date = _dt.date.today() + _dt.timedelta(days=min_dte)
        eligible_expiries = [d for d in expirations if d >= target_date]
        if not eligible_expiries:
            # All expirations are sooner than target — pick the furthest available
            expiry = expirations[-1]
            logger.info("No expiry >= %s, using furthest available: %s", target_date, expiry)
        else:
            expiry = eligible_expiries[0]
        dte = (expiry - _dt.date.today()).days
        logger.info("Selected expiry: %s (DTE=%d, target was >=%d)", expiry, dte, min_dte)

        # 4. Pick right: call if bullish, put if bearish
        right = "call" if thesis.direction == "bullish" else "put"

        # 5. Pick strike: target OTM by otm_fraction * expected_move_pct
        price_f = float(stock_price)
        otm_pct = thesis.expected_move_pct * self.config.otm_fraction
        otm_pct = min(otm_pct, self.config.max_otm_pct)

        if right == "call":
            target_strike = price_f * (1 + otm_pct / 100)
        else:
            target_strike = price_f * (1 - otm_pct / 100)

        # Sort candidate strikes by distance from target
        ranked_strikes = sorted(strikes, key=lambda s: abs(s - target_strike))
        # Keep only strikes within max_otm_pct
        candidate_strikes = [
            s for s in ranked_strikes
            if abs(s - price_f) / price_f * 100 <= self.config.max_otm_pct
        ]

        if not candidate_strikes:
            logger.warning("No strikes within %.1f%% OTM for %s", self.config.max_otm_pct, ticker)
            return None

        # 6. Try candidate strikes × expiries until one qualifies in IB
        #    Not every strike exists at every expiry, so we need to retry.
        expiry_candidates = eligible_expiries[:3] if eligible_expiries else [expirations[-1]]

        for exp in expiry_candidates:
            dte = (exp - _dt.date.today()).days
            for strike in candidate_strikes[:5]:  # Try up to 5 nearest strikes
                actual_otm_pct = abs(strike - price_f) / price_f * 100
                strike_dec = Decimal(str(strike))

                logger.info(
                    "Trying: %s %s%s exp %s (DTE=%d, OTM %.1f%%)",
                    ticker, strike_dec, right[0].upper(), exp, dte, actual_otm_pct,
                )

                temp_spec = ContractSpec(
                    ticker=ticker,
                    right=right,
                    strike=strike_dec,
                    expiry=exp,
                    entry_price_low=Decimal("0"),
                    entry_price_high=Decimal("999"),
                )

                try:
                    quote = await self.ib.get_option_quote(temp_spec)
                except Exception as e:
                    logger.info("  Strike $%s exp %s not available: %s", strike_dec, exp, e)
                    continue

                logger.info(
                    "Option quote %s %s%s exp %s: bid=$%s ask=$%s mid=$%s",
                    ticker, strike_dec, right[0].upper(), exp,
                    quote.bid, quote.ask, quote.mid,
                )

                # 7. Validate liquidity
                if quote.mid < self.config.min_mid_price:
                    logger.info("  Mid $%s < min $%s — skip", quote.mid, self.config.min_mid_price)
                    continue

                if quote.bid > 0 and quote.ask > 0 and quote.mid > 0:
                    spread_pct = float((quote.ask - quote.bid) / quote.mid)
                    if spread_pct > self.config.max_spread_pct:
                        logger.warning(
                            "Spread %.1f%% > max %.1f%% — proceeding with caution",
                            spread_pct * 100, self.config.max_spread_pct * 100,
                        )

                # 8. Build final ContractSpec with real prices
                entry_low = quote.bid if quote.bid > 0 else quote.mid * Decimal("0.95")
                entry_high = quote.ask if quote.ask > 0 else quote.mid * Decimal("1.05")

                spec = ContractSpec(
                    ticker=ticker,
                    right=right,
                    strike=strike_dec,
                    expiry=exp,
                    entry_price_low=entry_low,
                    entry_price_high=entry_high,
                )

                logger.info("Contract selected: %s", spec)
                return spec

        logger.warning("No valid contract found for %s after trying multiple strikes/expiries", ticker)
        return None
