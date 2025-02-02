use {
    crate::{errors::ErrorCode, state::*},
    anchor_lang::{
        prelude::*,
        solana_program::{
            program::{invoke, invoke_signed},
            system_instruction::transfer,
        },
        AccountsClose,
    },
    anchor_spl::token::{self, Approve, FreezeAccount, Mint, Token, TokenAccount, Transfer},
    mpl_token_metadata::{instruction::freeze_delegated_account, utils::assert_derivation},
};

#[derive(Accounts)]
pub struct ClaimCtx<'info> {
    #[account(mut, constraint = token_manager.state == TokenManagerState::Issued as u8 @ ErrorCode::InvalidTokenManagerState)]
    token_manager: Box<Account<'info, TokenManager>>,
    #[account(mut, constraint =
        token_manager_token_account.owner == token_manager.key()
        && token_manager_token_account.mint == token_manager.mint
        @ ErrorCode::InvalidTokenManagerTokenAccount
    )]
    token_manager_token_account: Box<Account<'info, TokenAccount>>,
    #[account(constraint = mint.key() == token_manager.mint @ ErrorCode::InvalidMint)]
    mint: Box<Account<'info, Mint>>,

    // recipient
    #[account(mut)]
    recipient: Signer<'info>,
    #[account(mut, constraint =
        recipient_token_account.owner == recipient.key()
        && recipient_token_account.mint == token_manager.mint
        @ ErrorCode::InvalidRecipientTokenAccount
    )]
    recipient_token_account: Box<Account<'info, TokenAccount>>,
    token_program: Program<'info, Token>,
    system_program: Program<'info, System>,
}

pub fn handler<'key, 'accounts, 'remaining, 'info>(ctx: Context<'key, 'accounts, 'remaining, 'info, ClaimCtx<'info>>) -> Result<()> {
    let token_manager = &mut ctx.accounts.token_manager;
    token_manager.recipient_token_account = ctx.accounts.recipient_token_account.key();
    token_manager.state = TokenManagerState::Claimed as u8;
    token_manager.state_changed_at = Clock::get().unwrap().unix_timestamp;
    let remaining_accs = &mut ctx.remaining_accounts.iter();

    // get PDA seeds to sign with
    let token_manager_seeds = &[TOKEN_MANAGER_SEED.as_bytes(), token_manager.mint.as_ref(), &[token_manager.bump]];
    let token_manager_signer = &[&token_manager_seeds[..]];

    // transfer amount to recipient token account
    let cpi_accounts = Transfer {
        from: ctx.accounts.token_manager_token_account.to_account_info(),
        to: ctx.accounts.recipient_token_account.to_account_info(),
        authority: token_manager.to_account_info(),
    };
    let cpi_program = ctx.accounts.token_program.to_account_info();
    let cpi_context = CpiContext::new(cpi_program, cpi_accounts).with_signer(token_manager_signer);
    token::transfer(cpi_context, token_manager.amount)?;

    // if this is a managed token, this means we will revoke it at the end of life, so we need to delegate and freeze
    match token_manager.kind {
        k if k == TokenManagerKind::Unmanaged as u8 => {}

        k if k == TokenManagerKind::Managed as u8 => {
            // set account delegate of recipient token account to token manager PDA
            let cpi_accounts = Approve {
                to: ctx.accounts.recipient_token_account.to_account_info(),
                delegate: token_manager.to_account_info(),
                authority: ctx.accounts.recipient.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_context = CpiContext::new(cpi_program, cpi_accounts);
            token::approve(cpi_context, token_manager.amount)?;

            let mint_manager_info = next_account_info(remaining_accs)?;
            let mint = ctx.accounts.mint.key();
            let path = &[MINT_MANAGER_SEED.as_bytes(), mint.as_ref()];
            assert_derivation(ctx.program_id, mint_manager_info, path)?;
            // update mint manager
            let mut mint_manager = Account::<MintManager>::try_from(mint_manager_info)?;
            mint_manager.token_managers = mint_manager.token_managers.checked_add(1).expect("Addition error");
            mint_manager.exit(ctx.program_id)?;
            let mint_manager_seeds = &[MINT_MANAGER_SEED.as_bytes(), mint.as_ref(), &[mint_manager.bump]];
            let mint_manager_signer = &[&mint_manager_seeds[..]];

            // freeze recipient token account
            let cpi_accounts = FreezeAccount {
                account: ctx.accounts.recipient_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                authority: mint_manager_info.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_context = CpiContext::new(cpi_program, cpi_accounts).with_signer(mint_manager_signer);
            token::freeze_account(cpi_context)?;
        }

        k if k == TokenManagerKind::Edition as u8 => {
            let edition_info = next_account_info(remaining_accs)?;
            let metadata_program = next_account_info(remaining_accs)?;

            // edition will be validated by metadata_program
            // assert_keys_eq!(metadata_program.key, mpl_token_metadata::id());
            if metadata_program.key() != mpl_token_metadata::id() {
                return Err(error!(ErrorCode::PublicKeyMismatch));
            }

            // set account delegate of recipient token account to token manager PDA
            let cpi_accounts = Approve {
                to: ctx.accounts.recipient_token_account.to_account_info(),
                delegate: token_manager.to_account_info(),
                authority: ctx.accounts.recipient.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_context = CpiContext::new(cpi_program, cpi_accounts);
            token::approve(cpi_context, token_manager.amount)?;

            invoke_signed(
                &freeze_delegated_account(
                    *metadata_program.key,
                    token_manager.key(),
                    ctx.accounts.recipient_token_account.key(),
                    *edition_info.key,
                    ctx.accounts.mint.key(),
                ),
                &[
                    token_manager.to_account_info(),
                    ctx.accounts.recipient_token_account.to_account_info(),
                    edition_info.to_account_info(),
                    ctx.accounts.mint.to_account_info(),
                ],
                &[token_manager_seeds],
            )?;
        }

        k if k == TokenManagerKind::Permissioned as u8 => {
            let mint_manager_info = next_account_info(remaining_accs)?;
            let mint = ctx.accounts.mint.key();
            let path = &[MINT_MANAGER_SEED.as_bytes(), mint.as_ref()];
            assert_derivation(ctx.program_id, mint_manager_info, path)?;

            // update mint manager
            let mut mint_manager = Account::<MintManager>::try_from(mint_manager_info)?;
            mint_manager.token_managers = mint_manager.token_managers.checked_add(1).expect("Addition error");
            mint_manager.exit(ctx.program_id)?;
            let mint_manager_seeds = &[MINT_MANAGER_SEED.as_bytes(), mint.as_ref(), &[mint_manager.bump]];
            let mint_manager_signer = &[&mint_manager_seeds[..]];

            // freeze recipient token account
            let cpi_accounts = FreezeAccount {
                account: ctx.accounts.recipient_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                authority: mint_manager_info.to_account_info(),
            };
            let cpi_program = ctx.accounts.token_program.to_account_info();
            let cpi_context = CpiContext::new(cpi_program, cpi_accounts).with_signer(mint_manager_signer);
            token::freeze_account(cpi_context)?;
        }
        _ => return Err(error!(ErrorCode::InvalidTokenManagerKind)),
    }

    if token_manager.invalidation_type == InvalidationType::Reissue as u8 || token_manager.invalidation_type == InvalidationType::Invalidate as u8 {
        invoke(
            &transfer(&ctx.accounts.recipient.key(), &token_manager.key(), INVALIDATION_REWARD_LAMPORTS),
            &[ctx.accounts.recipient.to_account_info(), token_manager.to_account_info(), ctx.accounts.system_program.to_account_info()],
        )?;
    }

    // verify claim receipt
    if token_manager.claim_approver.is_some() {
        let claim_receipt_info = next_account_info(remaining_accs)?;
        let claim_receipt = Account::<ClaimReceipt>::try_from(claim_receipt_info)?;
        if claim_receipt.mint_count != token_manager.count {
            return Err(error!(ErrorCode::InvalidClaimReceipt));
        }
        if claim_receipt.token_manager != token_manager.key() {
            return Err(error!(ErrorCode::InvalidClaimReceipt));
        }
        if claim_receipt.target != ctx.accounts.recipient.key() {
            return Err(error!(ErrorCode::InvalidClaimReceipt));
        }
        claim_receipt.close(token_manager.to_account_info())?;
    }
    Ok(())
}
