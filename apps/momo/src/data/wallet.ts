import type { WalletSnapshot } from "./types";

export const wallet: WalletSnapshot = {
  balance: "$2,418.32",
  monthlySpend: "$326.80",
  pendingHolds: "$42.00",
  kycStatus: "Verified",
  recentActivity: [
    {
      id: "a1",
      date: "Today 09:42",
      merchant: "Crypto top up",
      description: "USDC deposit from 0x91…4A2C · Confirmed",
      amount: "+$1,500.00",
      category: "Funding",
      direction: "in",
      time: "Today 09:42"
    },
    {
      id: "a2",
      date: "Today 10:16",
      merchant: "Agent paid Narisawa deposit",
      description: "Restaurant booking · Approved by Hanzhi",
      amount: "-$42.00",
      category: "Dining hold",
      direction: "out",
      time: "Today 10:16"
    },
    {
      id: "a3",
      date: "Yesterday",
      merchant: "Agent ordered birthday gift",
      description: "Rakuten order · Gift for Kim",
      amount: "-$128.50",
      category: "Gifts",
      direction: "out",
      time: "Yesterday"
    }
  ]
};
