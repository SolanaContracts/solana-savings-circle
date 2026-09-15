use anchor_lang::prelude::*;
use anchor_spl::associated_token::{get_associated_token_address, AssociatedToken};
use anchor_spl::token::{transfer, Mint, Token, TokenAccount, Transfer};

declare_id!("3vkUrEeswVjU2coEngV1fwdVFTnZuUo7QeZxSzBNdmhs");

pub const MAX_MEMBERS: u8 = 10;

#[program]
pub mod solana_savings_circle {
    use super::*;

    pub fn create_circle(
        ctx: Context<CreateCircle>,
        circle_id: u64,
        contribution_amount: u64,
        member_count: u8,
    ) -> Result<()> {
        require!(
            (2..=MAX_MEMBERS).contains(&member_count),
            CircleError::InvalidMemberCount
        );
        require!(contribution_amount > 0, CircleError::InvalidMemberCount);

        let circle = &mut ctx.accounts.circle;
        circle.creator = ctx.accounts.creator.key();
        circle.circle_id = circle_id;
        circle.mint = ctx.accounts.mint.key();
        circle.contribution_amount = contribution_amount;
        circle.member_count = member_count;
        circle.joined_count = 0;
        circle.members = [Pubkey::default(); MAX_MEMBERS as usize];
        circle.current_round = 0;
        circle.contributed_mask = 0;
        circle.paid_out_mask = 0;
        circle.started = false;
        circle.bump = ctx.bumps.circle;

        Ok(())
    }

    pub fn join_circle(ctx: Context<JoinCircle>) -> Result<()> {
        let circle = &mut ctx.accounts.circle;
        require!(!circle.started, CircleError::CircleFullError);
        require!(
            circle.joined_count < circle.member_count,
            CircleError::CircleFullError
        );
        require!(
            !circle.members[..circle.joined_count as usize].contains(&ctx.accounts.member.key()),
            CircleError::AlreadyMemberError
        );

        let idx = circle.joined_count as usize;
        circle.members[idx] = ctx.accounts.member.key();
        circle.joined_count += 1;

        if circle.joined_count == circle.member_count {
            circle.started = true;
            circle.current_round = 1;
        }

        Ok(())
    }

    pub fn contribute<'info>(ctx: Context<'_, '_, 'info, 'info, Contribute<'info>>) -> Result<()> {
        let member_key = ctx.accounts.member.key();

        {
            let circle = &ctx.accounts.circle;
            require!(circle.started, CircleError::CircleNotStartedError);
            require!(
                circle.current_round >= 1 && circle.current_round <= circle.member_count,
                CircleError::CircleNotStartedError
            );
        }

        let member_index = {
            let circle = &ctx.accounts.circle;
            circle.members[..circle.member_count as usize]
                .iter()
                .position(|m| *m == member_key)
                .ok_or(CircleError::NotAMemberError)?
        };

        {
            let circle = &ctx.accounts.circle;
            require!(
                circle.contributed_mask & (1 << member_index) == 0,
                CircleError::AlreadyContributedError
            );
        }

        require!(
            ctx.remaining_accounts.len() == ctx.accounts.circle.member_count as usize,
            CircleError::InvalidMemberTokenAccount
        );
        for (i, account_info) in ctx.remaining_accounts.iter().enumerate() {
            let circle = &ctx.accounts.circle;
            let expected = get_associated_token_address(&circle.members[i], &circle.mint);
            require_keys_eq!(
                account_info.key(),
                expected,
                CircleError::InvalidMemberTokenAccount
            );
        }

        transfer(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.member_token_account.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.member.to_account_info(),
                },
            ),
            ctx.accounts.circle.contribution_amount,
        )?;

        ctx.accounts.circle.contributed_mask |= 1 << member_index;

        let round_complete = {
            let circle = &ctx.accounts.circle;
            let full_mask = (1u16 << circle.member_count) - 1;
            circle.contributed_mask == full_mask
        };

        if round_complete {
            let (winner_index, pot_amount) = {
                let circle = &ctx.accounts.circle;
                let candidates: Vec<usize> = (0..circle.member_count as usize)
                    .filter(|i| circle.paid_out_mask & (1 << i) == 0)
                    .collect();
                require!(!candidates.is_empty(), CircleError::CircleNotComplete);

                // NOTE: pseudo-random only (seeded by the current slot). Good enough for a
                // learning project, but a validator/leader has some influence over slot timing,
                // so this is not manipulation-proof. A production deployment should use a
                // verifiable randomness source (e.g. Switchboard VRF) instead.
                let seed = Clock::get()?.slot as usize;
                let winner_index = candidates[seed % candidates.len()];

                let pot_amount = circle
                    .contribution_amount
                    .checked_mul(circle.member_count as u64)
                    .ok_or(CircleError::InvalidMemberCount)?;

                (winner_index, pot_amount)
            };

            let winner_account = &ctx.remaining_accounts[winner_index];

            let creator_key = ctx.accounts.circle.creator;
            let circle_id_bytes = ctx.accounts.circle.circle_id.to_le_bytes();
            let bump = ctx.accounts.circle.bump;
            let signer_seeds: &[&[u8]] = &[
                b"circle",
                creator_key.as_ref(),
                circle_id_bytes.as_ref(),
                &[bump],
            ];

            transfer(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    Transfer {
                        from: ctx.accounts.vault.to_account_info(),
                        to: winner_account.clone(),
                        authority: ctx.accounts.circle.to_account_info(),
                    },
                    &[signer_seeds],
                ),
                pot_amount,
            )?;

            let circle = &mut ctx.accounts.circle;
            circle.paid_out_mask |= 1 << winner_index;
            circle.contributed_mask = 0;
            circle.current_round += 1;
        }

        Ok(())
    }

    pub fn close_circle(ctx: Context<CloseCircle>) -> Result<()> {
        let circle = &ctx.accounts.circle;
        require!(
            circle.current_round == circle.member_count + 1,
            CircleError::CircleNotComplete
        );

        let creator_key = circle.creator;
        let circle_id_bytes = circle.circle_id.to_le_bytes();
        let bump = circle.bump;
        let signer_seeds: &[&[u8]] = &[
            b"circle",
            creator_key.as_ref(),
            circle_id_bytes.as_ref(),
            &[bump],
        ];

        anchor_spl::token::close_account(CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: ctx.accounts.vault.to_account_info(),
                destination: ctx.accounts.creator.to_account_info(),
                authority: ctx.accounts.circle.to_account_info(),
            },
            &[signer_seeds],
        ))?;

        Ok(())
    }
}

#[account]
pub struct SavingsCircle {
    pub creator: Pubkey,
    pub circle_id: u64,
    pub mint: Pubkey,
    pub contribution_amount: u64,
    pub member_count: u8,
    pub joined_count: u8,
    pub members: [Pubkey; MAX_MEMBERS as usize],
    pub current_round: u8,
    pub contributed_mask: u16,
    pub paid_out_mask: u16,
    pub started: bool,
    pub bump: u8,
}

impl SavingsCircle {
    pub const MAX_SIZE: usize = 8 // discriminator
        + 32 // creator
        + 8 // circle_id
        + 32 // mint
        + 8 // contribution_amount
        + 1 // member_count
        + 1 // joined_count
        + 32 * MAX_MEMBERS as usize // members
        + 1 // current_round
        + 2 // contributed_mask
        + 2 // paid_out_mask
        + 1 // started
        + 1; // bump
}

#[derive(Accounts)]
#[instruction(circle_id: u64)]
pub struct CreateCircle<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(
        init,
        payer = creator,
        space = SavingsCircle::MAX_SIZE,
        seeds = [b"circle", creator.key().as_ref(), circle_id.to_le_bytes().as_ref()],
        bump,
    )]
    pub circle: Account<'info, SavingsCircle>,

    pub mint: Account<'info, Mint>,

    #[account(
        init,
        payer = creator,
        token::mint = mint,
        token::authority = circle,
        seeds = [b"vault", circle.key().as_ref()],
        bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct JoinCircle<'info> {
    #[account(mut)]
    pub member: Signer<'info>,

    #[account(mut)]
    pub circle: Account<'info, SavingsCircle>,

    #[account(address = circle.mint)]
    pub mint: Account<'info, Mint>,

    #[account(
        init_if_needed,
        payer = member,
        associated_token::mint = mint,
        associated_token::authority = member,
    )]
    pub member_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Contribute<'info> {
    #[account(mut)]
    pub member: Signer<'info>,

    #[account(
        mut,
        seeds = [b"circle", circle.creator.as_ref(), circle.circle_id.to_le_bytes().as_ref()],
        bump = circle.bump,
    )]
    pub circle: Account<'info, SavingsCircle>,

    #[account(
        mut,
        associated_token::mint = circle.mint,
        associated_token::authority = member,
    )]
    pub member_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"vault", circle.key().as_ref()],
        bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    // remaining_accounts: every member's ATA, in the same order as `circle.members`,
    // so a payout winner (chosen at runtime) can be paid in this same instruction.
}

#[derive(Accounts)]
pub struct CloseCircle<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(
        mut,
        has_one = creator @ CircleError::NotAMemberError,
        seeds = [b"circle", circle.creator.as_ref(), circle.circle_id.to_le_bytes().as_ref()],
        bump = circle.bump,
        close = creator,
    )]
    pub circle: Account<'info, SavingsCircle>,

    #[account(
        mut,
        seeds = [b"vault", circle.key().as_ref()],
        bump,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

#[error_code]
pub enum CircleError {
    #[msg("Circle is already full or has already started")]
    CircleFullError,
    #[msg("This account is already a member of the circle")]
    AlreadyMemberError,
    #[msg("Circle has not started yet (not all seats are filled)")]
    CircleNotStartedError,
    #[msg("Signer is not a member of this circle")]
    NotAMemberError,
    #[msg("This member has already contributed for the current round")]
    AlreadyContributedError,
    #[msg("A provided token account does not match the expected member ATA")]
    InvalidMemberTokenAccount,
    #[msg("Circle has not completed all rounds yet")]
    CircleNotComplete,
    #[msg("member_count must be between 2 and MAX_MEMBERS, with a positive contribution_amount")]
    InvalidMemberCount,
}
