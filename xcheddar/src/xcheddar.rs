
use crate::*;
#[allow(unused_imports)]
use crate::utils::*;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_contract_standards::fungible_token::core_impl::ext_fungible_token;
use near_sdk::json_types::U128;
use near_sdk::{assert_one_yocto, env, log, Promise, PromiseResult};
use std::cmp::{max, min};

impl Contract {
    pub fn internal_stake(&mut self, account_id: &AccountId, amount: Balance) {
        // check account has registered
        assert!(self.ft.accounts.contains_key(account_id), "Account @{} is not registered", account_id);
        
        let mut minted = amount;

        if self.ft.total_supply != 0 {
            assert!(self.locked_token_amount > 0, "{}", ERR_INTERNAL);
            minted = (U256::from(amount) * U256::from(self.ft.total_supply) / U256::from(self.locked_token_amount)).as_u128();
        }
        
        assert!(minted > 0, "{}", ERR_STAKE_TOO_SMALL);

        self.locked_token_amount += amount;

        self.ft.internal_deposit(account_id, minted);
        log!("@{} Stake {} (~{} CHEDDAR) assets, get {} (~{} xCHEDDAR) tokens",
            account_id, 
            amount,
            convert_from_yocto_cheddar(amount),
            minted,
            convert_from_yocto_cheddar(minted)
        );
    }

    pub fn internal_add_reward(&mut self, account_id: &AccountId, amount: Balance) {
        self.undistributed_reward += amount;
        log!("@{} add {} (~{} CHEDDAR) assets as reward", account_id, amount, convert_from_yocto_cheddar(amount));
    }

    // return the amount of to be distribute reward this time
    pub(crate) fn try_distribute_reward(&self, cur_timestamp_in_sec: u32) -> Balance {
        if cur_timestamp_in_sec > self.reward_genesis_time_in_sec && cur_timestamp_in_sec > self.prev_distribution_time_in_sec {
            //reward * (duration between previous distribution and current time)
            //reward_per_month = reward_per_sec * DURATION_30_DAYS_IN_SEC
            let ideal_amount = self.monthly_reward * ((cur_timestamp_in_sec - self.prev_distribution_time_in_sec) / DURATION_30DAYS_IN_SEC) as u128;
            min(ideal_amount, self.undistributed_reward)
        } else {
            0
        }
    }

    pub(crate) fn distribute_reward(&mut self) {
        let cur_time = nano_to_sec(env::block_timestamp());
        let new_reward = self.try_distribute_reward(cur_time);
        if new_reward > 0 {
            self.undistributed_reward -= new_reward;
            self.locked_token_amount += new_reward;
        }
        self.prev_distribution_time_in_sec = max(cur_time, self.reward_genesis_time_in_sec);
    }
}

#[near_bindgen]
impl Contract {
    /// unstake token and send assets back to the predecessor account.
    /// Requirements:
    /// * The predecessor account should be registered.
    /// * `amount` must be a positive integer.
    /// * The predecessor account should have at least the `amount` of tokens.
    /// * Requires attached deposit of exactly 1 yoctoNEAR.
    /// ? : withdraw on every time or it opens in windows?
    #[payable]
    pub fn unstake(&mut self, amount: U128) -> Promise {
        // Checkpoint
        self.distribute_reward();

        assert_one_yocto();

        let account_id = env::predecessor_account_id();
        let amount: Balance = amount.into();

        assert!(self.ft.total_supply > 0, "{}", ERR_EMPTY_TOTAL_SUPPLY);
        let unlocked = (U256::from(amount) * U256::from(self.locked_token_amount) / U256::from(self.ft.total_supply)).as_u128();

        self.ft.internal_withdraw(&account_id, amount);
        assert!(self.ft.total_supply >= 10u128.pow(24), "{}", ERR_KEEP_AT_LEAST_ONE_XCHEDDAR);
        self.locked_token_amount -= unlocked;

        log!("Withdraw {} (~{} Cheddar) from @{}", amount, convert_from_yocto_cheddar(amount), account_id);

        ext_fungible_token::ft_transfer(
            account_id.clone(),
            U128(unlocked),
            None,
            self.locked_token.clone(),
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_self::callback_post_unstake(
            account_id.clone(),
            U128(unlocked),
            U128(amount),
            env::current_account_id(),
            NO_DEPOSIT,
            GAS_FOR_RESOLVE_TRANSFER,
        ))
    }

    #[private]
    pub fn callback_post_unstake(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        share: U128,
    ) {
        assert_eq!(
            env::promise_results_count(),
            1,
            "{}", ERR_PROMISE_RESULT
        );
        match env::promise_result(0) {
            PromiseResult::NotReady => unreachable!(),
            PromiseResult::Successful(_) => {
                log!(
                        "Account @{} successful unstake {} (~{} CHEDDAR).",
                        sender_id,
                        amount.0,
                        convert_from_yocto_cheddar(amount.0)
                    );
            }
            PromiseResult::Failed => {
                // This reverts the changes from unstake function.
                // If account doesn't exit, the unlock token stay in contract.
                if self.ft.accounts.contains_key(&sender_id) {
                    self.locked_token_amount += amount.0;
                    self.ft.internal_deposit(&sender_id, share.0);
                    log!(
                            "Account @{} unstake failed and reverted.",
                            sender_id
                        );
                } else {
                    log!(
                            "Account @{} has unregistered. Unlocking token goes to contract.",
                            sender_id
                        );
                }
            }
        };
    }
}

#[near_bindgen]
impl FungibleTokenReceiver for Contract {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        // Checkpoint
        self.distribute_reward();
        let token_in = env::predecessor_account_id();
        let amount: Balance = amount.into();
        assert_eq!(token_in, self.locked_token, "{}", ERR_MISMATCH_TOKEN);
        if msg.is_empty() {
            // user stake
            self.internal_stake(&sender_id, amount);
            PromiseOrValue::Value(U128(0))
        } else {
            // deposit reward
            log!("Add reward {} token with msg {}", amount, msg);
            self.internal_add_reward(&sender_id, amount);
            PromiseOrValue::Value(U128(0))
        }
    }
}