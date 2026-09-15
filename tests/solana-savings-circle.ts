import * as anchor from "@coral-xyz/anchor";
import { BN, Program } from "@coral-xyz/anchor";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  LAMPORTS_PER_SOL,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID,
  createMint,
  mintTo,
  createAssociatedTokenAccount,
  getAssociatedTokenAddressSync,
  getAccount,
} from "@solana/spl-token";
import { assert } from "chai";
import { SolanaSavingsCircle } from "../target/types/solana_savings_circle";

describe("solana-savings-circle", () => {
  anchor.setProvider(anchor.AnchorProvider.env());
  const provider = anchor.getProvider() as anchor.AnchorProvider;
  const program = anchor.workspace
    .solanaSavingsCircle as Program<SolanaSavingsCircle>;

  const organizer = Keypair.generate();
  const members = [Keypair.generate(), Keypair.generate(), Keypair.generate()];
  const MEMBER_COUNT = members.length;
  const DECIMALS = 6;
  const CONTRIBUTION_AMOUNT = 10 * 10 ** DECIMALS;
  const CIRCLE_ID = new BN(1);

  let mint: PublicKey;
  let circlePda: PublicKey;
  let vaultPda: PublicKey;

  const findCirclePda = (creator: PublicKey, circleId: BN) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("circle"), creator.toBuffer(), circleId.toArrayLike(Buffer, "le", 8)],
      program.programId
    )[0];

  const findVaultPda = (circle: PublicKey) =>
    PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), circle.toBuffer()],
      program.programId
    )[0];

  const memberAta = (member: PublicKey) =>
    getAssociatedTokenAddressSync(mint, member);

  const orderedMemberMetas = async () => {
    const circle = await program.account.savingsCircle.fetch(circlePda);
    return circle.members.slice(0, MEMBER_COUNT).map((pubkey: PublicKey) => ({
      pubkey: memberAta(pubkey),
      isWritable: true,
      isSigner: false,
    }));
  };

  before(async () => {
    for (const kp of [organizer, ...members]) {
      const sig = await provider.connection.requestAirdrop(
        kp.publicKey,
        2 * LAMPORTS_PER_SOL
      );
      await provider.connection.confirmTransaction(sig, "confirmed");
    }

    mint = await createMint(
      provider.connection,
      organizer,
      organizer.publicKey,
      null,
      DECIMALS
    );

    circlePda = findCirclePda(organizer.publicKey, CIRCLE_ID);
    vaultPda = findVaultPda(circlePda);
  });

  it("rejects an invalid member count", async () => {
    const badId = new BN(999);
    const badCirclePda = findCirclePda(organizer.publicKey, badId);
    const badVaultPda = findVaultPda(badCirclePda);

    try {
      await program.methods
        .createCircle(badId, new BN(CONTRIBUTION_AMOUNT), 1)
        .accounts({
          creator: organizer.publicKey,
          circle: badCirclePda,
          mint,
          vault: badVaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .signers([organizer])
        .rpc();
      assert.fail("expected create_circle to fail");
    } catch (err) {
      assert.include(String(err), "InvalidMemberCount");
    }
  });

  it("creates a circle", async () => {
    await program.methods
      .createCircle(CIRCLE_ID, new BN(CONTRIBUTION_AMOUNT), MEMBER_COUNT)
      .accounts({
        creator: organizer.publicKey,
        circle: circlePda,
        mint,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      })
      .signers([organizer])
      .rpc();

    const circle = await program.account.savingsCircle.fetch(circlePda);
    assert.equal(circle.memberCount, MEMBER_COUNT);
    assert.equal(circle.joinedCount, 0);
    assert.isFalse(circle.started);
  });

  it("lets every member join, auto-starting once full", async () => {
    for (const member of members) {
      await program.methods
        .joinCircle()
        .accounts({
          member: member.publicKey,
          circle: circlePda,
          mint,
          memberTokenAccount: memberAta(member.publicKey),
          tokenProgram: TOKEN_PROGRAM_ID,
          associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
          systemProgram: SystemProgram.programId,
          rent: SYSVAR_RENT_PUBKEY,
        })
        .signers([member])
        .rpc();
    }

    const circle = await program.account.savingsCircle.fetch(circlePda);
    assert.equal(circle.joinedCount, MEMBER_COUNT);
    assert.isTrue(circle.started);
    assert.equal(circle.currentRound, 1);

    // fund every member with enough tokens for all MEMBER_COUNT rounds
    for (const member of members) {
      await mintTo(
        provider.connection,
        organizer,
        mint,
        memberAta(member.publicKey),
        organizer,
        CONTRIBUTION_AMOUNT * MEMBER_COUNT
      );
    }
  });

  it("rejects a contribution from a non-member", async () => {
    const outsider = Keypair.generate();
    const sig = await provider.connection.requestAirdrop(
      outsider.publicKey,
      LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig, "confirmed");
    await createAssociatedTokenAccount(
      provider.connection,
      outsider,
      mint,
      outsider.publicKey
    );

    try {
      await program.methods
        .contribute()
        .accounts({
          member: outsider.publicKey,
          circle: circlePda,
          memberTokenAccount: memberAta(outsider.publicKey),
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts(await orderedMemberMetas())
        .signers([outsider])
        .rpc();
      assert.fail("expected contribute to fail");
    } catch (err) {
      assert.include(String(err), "NotAMemberError");
    }
  });

  it("rejects a contribution with a mismatched remaining_accounts list", async () => {
    const metas = await orderedMemberMetas();
    const tampered = [metas[1], metas[0], metas[2]]; // wrong order

    try {
      await program.methods
        .contribute()
        .accounts({
          member: members[0].publicKey,
          circle: circlePda,
          memberTokenAccount: memberAta(members[0].publicKey),
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts(tampered)
        .signers([members[0]])
        .rpc();
      assert.fail("expected contribute to fail");
    } catch (err) {
      assert.include(String(err), "InvalidMemberTokenAccount");
    }
  });

  it("rejects closing the circle before it's complete", async () => {
    try {
      await program.methods
        .closeCircle()
        .accounts({
          creator: organizer.publicKey,
          circle: circlePda,
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .signers([organizer])
        .rpc();
      assert.fail("expected close_circle to fail");
    } catch (err) {
      assert.include(String(err), "CircleNotComplete");
    }
  });

  it("rejects a duplicate contribution in the same round", async () => {
    const metas = await orderedMemberMetas();

    await program.methods
      .contribute()
      .accounts({
        member: members[0].publicKey,
        circle: circlePda,
        memberTokenAccount: memberAta(members[0].publicKey),
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .remainingAccounts(metas)
      .signers([members[0]])
      .rpc();

    try {
      await program.methods
        .contribute()
        .accounts({
          member: members[0].publicKey,
          circle: circlePda,
          memberTokenAccount: memberAta(members[0].publicKey),
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts(metas)
        .signers([members[0]])
        .rpc();
      assert.fail("expected duplicate contribute to fail");
    } catch (err) {
      assert.include(String(err), "AlreadyContributedError");
    }

    // finish round 1 with the remaining two members
    for (const member of [members[1], members[2]]) {
      await program.methods
        .contribute()
        .accounts({
          member: member.publicKey,
          circle: circlePda,
          memberTokenAccount: memberAta(member.publicKey),
          vault: vaultPda,
          tokenProgram: TOKEN_PROGRAM_ID,
        })
        .remainingAccounts(metas)
        .signers([member])
        .rpc();
    }

    const circle = await program.account.savingsCircle.fetch(circlePda);
    assert.equal(circle.currentRound, 2);
    assert.equal(circle.contributedMask, 0);
    // exactly one member has won so far
    const winners = [0, 1, 2].filter((i) => (circle.paidOutMask & (1 << i)) !== 0);
    assert.equal(winners.length, 1);
  });

  it("runs the remaining rounds to completion and lets the last winner be paid via remaining_accounts", async () => {
    const metas = await orderedMemberMetas();

    // round 2
    for (const member of members) {
      const circle = await program.account.savingsCircle.fetch(circlePda);
      if (circle.currentRound > MEMBER_COUNT) break;
      try {
        await program.methods
          .contribute()
          .accounts({
            member: member.publicKey,
            circle: circlePda,
            memberTokenAccount: memberAta(member.publicKey),
            vault: vaultPda,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .remainingAccounts(metas)
          .signers([member])
          .rpc();
      } catch (_) {
        // already contributed this round from a previous test iteration; ignore
      }
    }

    // round 3 (final round for a 3-member circle)
    for (const member of members) {
      const circle = await program.account.savingsCircle.fetch(circlePda);
      if (circle.currentRound > MEMBER_COUNT) break;
      try {
        await program.methods
          .contribute()
          .accounts({
            member: member.publicKey,
            circle: circlePda,
            memberTokenAccount: memberAta(member.publicKey),
            vault: vaultPda,
            tokenProgram: TOKEN_PROGRAM_ID,
          })
          .remainingAccounts(metas)
          .signers([member])
          .rpc();
      } catch (_) {
        // ignore double-contribute attempts across the loop
      }
    }

    const circle = await program.account.savingsCircle.fetch(circlePda);
    assert.equal(circle.currentRound, MEMBER_COUNT + 1);
    assert.equal(circle.paidOutMask, (1 << MEMBER_COUNT) - 1);

    // Each member contributed CONTRIBUTION_AMOUNT once per round across all
    // MEMBER_COUNT rounds (their whole starting balance), and won the full pot
    // (CONTRIBUTION_AMOUNT * MEMBER_COUNT) exactly once -> everyone ends back at
    // their starting balance. That's the point of a fair rotation: nobody is up
    // or down at the end, they just got the lump sum on a different round.
    for (const member of members) {
      const account = await getAccount(provider.connection, memberAta(member.publicKey));
      assert.equal(account.amount.toString(), String(CONTRIBUTION_AMOUNT * MEMBER_COUNT));
    }

    const vaultAccount = await getAccount(provider.connection, vaultPda);
    assert.equal(vaultAccount.amount.toString(), "0");
  });

  it("closes the completed circle and reclaims rent", async () => {
    await program.methods
      .closeCircle()
      .accounts({
        creator: organizer.publicKey,
        circle: circlePda,
        vault: vaultPda,
        tokenProgram: TOKEN_PROGRAM_ID,
      })
      .signers([organizer])
      .rpc();

    const closedCircle = await program.account.savingsCircle.fetchNullable(circlePda);
    assert.isNull(closedCircle);

    const vaultInfo = await provider.connection.getAccountInfo(vaultPda);
    assert.isNull(vaultInfo);
  });
});
