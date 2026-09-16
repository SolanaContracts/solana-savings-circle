use std::path::PathBuf;
use std::rc::Rc;

use anchor_client::{
    solana_sdk::{
        commitment_config::CommitmentConfig,
        instruction::AccountMeta,
        pubkey::Pubkey,
        signature::{read_keypair_file, Keypair, Signer},
    },
    Client, Cluster,
};
use anchor_spl::associated_token::get_associated_token_address;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use solana_savings_circle::{accounts, instruction, SavingsCircle};

#[derive(Parser)]
#[command(name = "circle-cli", about = "CLI client for the solana-savings-circle Anchor program")]
struct Cli {
    /// JSON-RPC URL of the cluster to talk to
    #[arg(long, global = true, default_value = "http://127.0.0.1:8899")]
    url: String,

    /// WebSocket URL of the cluster (used for transaction confirmation)
    #[arg(long, global = true, default_value = "ws://127.0.0.1:8900")]
    ws_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new savings circle
    CreateCircle {
        /// Keypair file for the circle creator (pays for setup)
        #[arg(long)]
        keypair: PathBuf,
        /// Arbitrary id so one creator can start multiple circles
        #[arg(long)]
        circle_id: u64,
        /// SPL token mint used for contributions
        #[arg(long)]
        mint: Pubkey,
        /// Contribution amount per member per round, in the mint's base units
        #[arg(long)]
        contribution_amount: u64,
        /// Number of members (2-10)
        #[arg(long)]
        member_count: u8,
    },
    /// Join an open circle
    JoinCircle {
        /// Keypair file for the joining member
        #[arg(long)]
        keypair: PathBuf,
        /// Circle creator's public key
        #[arg(long)]
        creator: Pubkey,
        #[arg(long)]
        circle_id: u64,
    },
    /// Pay this round's contribution; auto-pays the round's winner once everyone has paid
    Contribute {
        /// Keypair file for the contributing member
        #[arg(long)]
        keypair: PathBuf,
        /// Circle creator's public key
        #[arg(long)]
        creator: Pubkey,
        #[arg(long)]
        circle_id: u64,
    },
    /// Close a completed circle and reclaim rent (creator only)
    CloseCircle {
        /// Keypair file for the circle creator
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        circle_id: u64,
    },
    /// Print a circle's current on-chain state
    Show {
        /// Circle creator's public key
        #[arg(long)]
        creator: Pubkey,
        #[arg(long)]
        circle_id: u64,
    },
}

fn circle_pda(creator: &Pubkey, circle_id: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[b"circle", creator.as_ref(), &circle_id.to_le_bytes()],
        &solana_savings_circle::ID,
    )
    .0
}

fn vault_pda(circle: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"vault", circle.as_ref()], &solana_savings_circle::ID).0
}

fn load_keypair(path: &PathBuf) -> Result<Keypair> {
    read_keypair_file(path)
        .map_err(|e| anyhow::anyhow!("failed to read keypair at {}: {e}", path.display()))
}

fn mask_to_indices(mask: u16, count: u8) -> Vec<u8> {
    (0..count).filter(|i| mask & (1 << i) != 0).collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cluster = Cluster::Custom(cli.url.clone(), cli.ws_url.clone());

    match cli.command {
        Command::CreateCircle {
            keypair,
            circle_id,
            mint,
            contribution_amount,
            member_count,
        } => {
            let creator = Rc::new(load_keypair(&keypair)?);
            let client = Client::new_with_options(cluster, creator.clone(), CommitmentConfig::confirmed());
            let program = client.program(solana_savings_circle::ID)?;
            let circle = circle_pda(&creator.pubkey(), circle_id);
            let vault = vault_pda(&circle);

            let sig = program
                .request()
                .accounts(accounts::CreateCircle {
                    creator: creator.pubkey(),
                    circle,
                    mint,
                    vault,
                    token_program: anchor_spl::token::ID,
                    system_program: anchor_client::solana_sdk::system_program::ID,
                    rent: anchor_client::solana_sdk::sysvar::rent::ID,
                })
                .args(instruction::CreateCircle {
                    circle_id,
                    contribution_amount,
                    member_count,
                })
                .send()
                .context("create_circle transaction failed")?;

            println!("Circle created at {circle}");
            println!("Vault: {vault}");
            println!("Signature: {sig}");
        }

        Command::JoinCircle {
            keypair,
            creator,
            circle_id,
        } => {
            let member = Rc::new(load_keypair(&keypair)?);
            let client = Client::new_with_options(cluster, member.clone(), CommitmentConfig::confirmed());
            let program = client.program(solana_savings_circle::ID)?;
            let circle = circle_pda(&creator, circle_id);
            let circle_state: SavingsCircle = program
                .account(circle)
                .context("failed to fetch circle (does it exist?)")?;

            let member_token_account = get_associated_token_address(&member.pubkey(), &circle_state.mint);

            let sig = program
                .request()
                .accounts(accounts::JoinCircle {
                    member: member.pubkey(),
                    circle,
                    mint: circle_state.mint,
                    member_token_account,
                    token_program: anchor_spl::token::ID,
                    associated_token_program: anchor_spl::associated_token::ID,
                    system_program: anchor_client::solana_sdk::system_program::ID,
                    rent: anchor_client::solana_sdk::sysvar::rent::ID,
                })
                .args(instruction::JoinCircle {})
                .send()
                .context("join_circle transaction failed")?;

            println!("Joined circle {circle}");
            println!("Signature: {sig}");
        }

        Command::Contribute {
            keypair,
            creator,
            circle_id,
        } => {
            let member = Rc::new(load_keypair(&keypair)?);
            let client = Client::new_with_options(cluster, member.clone(), CommitmentConfig::confirmed());
            let program = client.program(solana_savings_circle::ID)?;
            let circle = circle_pda(&creator, circle_id);
            let vault = vault_pda(&circle);
            let circle_state: SavingsCircle = program
                .account(circle)
                .context("failed to fetch circle (does it exist?)")?;

            let member_token_account = get_associated_token_address(&member.pubkey(), &circle_state.mint);
            let remaining: Vec<AccountMeta> = circle_state.members
                [..circle_state.member_count as usize]
                .iter()
                .map(|m| {
                    AccountMeta::new(get_associated_token_address(m, &circle_state.mint), false)
                })
                .collect();

            let sig = program
                .request()
                .accounts(accounts::Contribute {
                    member: member.pubkey(),
                    circle,
                    member_token_account,
                    vault,
                    token_program: anchor_spl::token::ID,
                })
                .accounts(remaining)
                .args(instruction::Contribute {})
                .send()
                .context("contribute transaction failed")?;

            println!("Contributed to round {}", circle_state.current_round);
            println!("Signature: {sig}");
        }

        Command::CloseCircle { keypair, circle_id } => {
            let creator = Rc::new(load_keypair(&keypair)?);
            let client = Client::new_with_options(cluster, creator.clone(), CommitmentConfig::confirmed());
            let program = client.program(solana_savings_circle::ID)?;
            let circle = circle_pda(&creator.pubkey(), circle_id);
            let vault = vault_pda(&circle);

            let sig = program
                .request()
                .accounts(accounts::CloseCircle {
                    creator: creator.pubkey(),
                    circle,
                    vault,
                    token_program: anchor_spl::token::ID,
                })
                .args(instruction::CloseCircle {})
                .send()
                .context("close_circle transaction failed")?;

            println!("Closed circle {circle}");
            println!("Signature: {sig}");
        }

        Command::Show { creator, circle_id } => {
            let dummy_payer = Rc::new(Keypair::new());
            let client = Client::new_with_options(cluster, dummy_payer, CommitmentConfig::confirmed());
            let program = client.program(solana_savings_circle::ID)?;
            let circle = circle_pda(&creator, circle_id);
            let circle_state: SavingsCircle = program
                .account(circle)
                .context("failed to fetch circle (does it exist?)")?;

            println!("Circle: {circle}");
            println!("  creator:          {}", circle_state.creator);
            println!("  mint:             {}", circle_state.mint);
            println!("  contribution:     {} (base units)", circle_state.contribution_amount);
            println!(
                "  members:          {}/{} joined",
                circle_state.joined_count, circle_state.member_count
            );
            for i in 0..circle_state.joined_count as usize {
                println!("    [{i}] {}", circle_state.members[i]);
            }
            println!("  started:          {}", circle_state.started);
            println!(
                "  round:            {} of {}",
                circle_state.current_round, circle_state.member_count
            );
            println!(
                "  paid this round:  {:?}",
                mask_to_indices(circle_state.contributed_mask, circle_state.member_count)
            );
            println!(
                "  already won:      {:?}",
                mask_to_indices(circle_state.paid_out_mask, circle_state.member_count)
            );
        }
    }

    Ok(())
}
