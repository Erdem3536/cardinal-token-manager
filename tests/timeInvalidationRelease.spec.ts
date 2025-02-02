import { BN } from "@project-serum/anchor";
import { expectTXTable } from "@saberhq/chai-solana";
import {
  SignerWallet,
  SolanaProvider,
  TransactionEnvelope,
} from "@saberhq/solana-contrib";
import type { Token } from "@solana/spl-token";
import type { PublicKey } from "@solana/web3.js";
import { Keypair, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { expect } from "chai";

import { findAta, invalidate, rentals, tryGetAccount } from "../src";
import { tokenManager } from "../src/programs";
import {
  InvalidationType,
  TokenManagerState,
} from "../src/programs/tokenManager";
import { createMint } from "./utils";
import { getProvider } from "./workspace";

describe("Time invalidation release", () => {
  const recipient = Keypair.generate();
  const user = Keypair.generate();
  let issuerTokenAccountId: PublicKey;
  let rentalMint: Token;

  before(async () => {
    const provider = getProvider();
    const airdropCreator = await provider.connection.requestAirdrop(
      user.publicKey,
      LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropCreator);

    const airdropRecipient = await provider.connection.requestAirdrop(
      recipient.publicKey,
      LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(airdropRecipient);

    // create rental mint
    [issuerTokenAccountId, rentalMint] = await createMint(
      provider.connection,
      user,
      user.publicKey,
      1,
      user.publicKey
    );
  });

  it("Create rental", async () => {
    const provider = getProvider();
    const [transaction, tokenManagerId] = await rentals.createRental(
      provider.connection,
      new SignerWallet(user),
      {
        timeInvalidation: { maxExpiration: Date.now() / 1000 + 1 },
        mint: rentalMint.publicKey,
        issuerTokenAccountId: issuerTokenAccountId,
        amount: new BN(1),
        invalidationType: InvalidationType.Release,
      }
    );
    const txEnvelope = new TransactionEnvelope(
      SolanaProvider.init({
        connection: provider.connection,
        wallet: new SignerWallet(user),
        opts: provider.opts,
      }),
      [...transaction.instructions]
    );
    await expectTXTable(txEnvelope, "create", {
      verbosity: "error",
      formatLogs: true,
    }).to.be.fulfilled;

    const tokenManagerData = await tokenManager.accounts.getTokenManager(
      provider.connection,
      tokenManagerId
    );
    expect(tokenManagerData.parsed.state).to.eq(TokenManagerState.Issued);
    expect(tokenManagerData.parsed.amount.toNumber()).to.eq(1);
    expect(tokenManagerData.parsed.mint).to.eqAddress(rentalMint.publicKey);
    expect(tokenManagerData.parsed.invalidators).length.greaterThanOrEqual(1);
    expect(tokenManagerData.parsed.issuer).to.eqAddress(user.publicKey);

    const checkIssuerTokenAccount = await rentalMint.getAccountInfo(
      issuerTokenAccountId
    );
    expect(checkIssuerTokenAccount.amount.toNumber()).to.eq(0);
  });

  it("Claim rental", async () => {
    const provider = getProvider();

    const tokenManagerId = await tokenManager.pda.tokenManagerAddressFromMint(
      provider.connection,
      rentalMint.publicKey
    );

    const transaction = await rentals.claimRental(
      provider.connection,
      new SignerWallet(recipient),
      tokenManagerId
    );

    const txEnvelope = new TransactionEnvelope(
      SolanaProvider.init({
        connection: provider.connection,
        wallet: new SignerWallet(recipient),
        opts: provider.opts,
      }),
      [...transaction.instructions]
    );

    await expectTXTable(txEnvelope, "claim", {
      verbosity: "error",
      formatLogs: true,
    }).to.be.fulfilled;

    const tokenManagerData = await tokenManager.accounts.getTokenManager(
      provider.connection,
      tokenManagerId
    );
    expect(tokenManagerData.parsed.state).to.eq(TokenManagerState.Claimed);
    expect(tokenManagerData.parsed.amount.toNumber()).to.eq(1);

    const checkIssuerTokenAccount = await rentalMint.getAccountInfo(
      issuerTokenAccountId
    );
    expect(checkIssuerTokenAccount.amount.toNumber()).to.eq(0);

    const checkRecipientTokenAccount = await rentalMint.getAccountInfo(
      await findAta(rentalMint.publicKey, recipient.publicKey)
    );
    expect(checkRecipientTokenAccount.amount.toNumber()).to.eq(1);
    expect(checkRecipientTokenAccount.isFrozen).to.eq(true);
  });

  it("Invalidate", async () => {
    await new Promise((r) => setTimeout(r, 2000));

    const provider = getProvider();
    const transaction = await invalidate(
      provider.connection,
      new SignerWallet(recipient),
      rentalMint.publicKey
    );

    const txEnvelope = new TransactionEnvelope(
      SolanaProvider.init({
        connection: provider.connection,
        wallet: new SignerWallet(recipient),
        opts: provider.opts,
      }),
      [...transaction.instructions]
    );

    await expectTXTable(txEnvelope, "use", {
      verbosity: "error",
      formatLogs: true,
    }).to.be.fulfilled;

    const tokenManagerId = await tokenManager.pda.tokenManagerAddressFromMint(
      provider.connection,
      rentalMint.publicKey
    );

    const tokenManagerData = await tryGetAccount(() =>
      tokenManager.accounts.getTokenManager(provider.connection, tokenManagerId)
    );
    expect(tokenManagerData).to.eq(null);

    const checkIssuerTokenAccount = await rentalMint.getAccountInfo(
      issuerTokenAccountId
    );
    expect(checkIssuerTokenAccount.amount.toNumber()).to.eq(0);

    const checkRecipientTokenAccount = await rentalMint.getAccountInfo(
      await findAta(rentalMint.publicKey, recipient.publicKey)
    );
    expect(checkRecipientTokenAccount.amount.toNumber()).to.eq(1);
    expect(checkRecipientTokenAccount.isFrozen).to.eq(false);
    expect(checkRecipientTokenAccount.delegatedAmount.toNumber()).to.eq(0);
    expect(checkRecipientTokenAccount.delegate).to.eq(null);
  });
});
