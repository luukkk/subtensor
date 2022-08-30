use super::*;
use sp_std::convert::TryInto;
use substrate_fixed::types::I65F63;
use substrate_fixed::transcendental::exp;
use substrate_fixed::transcendental::log2;
use frame_support::IterableStorageMap;

const LOG_TARGET: &'static str = "runtime::subtensor::step";

impl<T: Config> Pallet<T> {

    pub fn update_difficulty() {

        // --- Set registrations per block to 0 after each block.
        RegistrationsThisBlock::<T>::set( 0 );

        // --- Difficulty adjustment constants for registration.
        let max_difficulty: u64 = Self::get_maximum_difficulty(); // Difficulty should never exceed this value.
        let min_difficulty: u64 = Self::get_minimum_difficulty(); // Difficulty should never be lower than this value.
        let adjustment_interval: u64 = Self::get_adjustment_interval(); // Number of blocks average registrations are taken over.
        let current_difficulty: u64 = Self::get_difficulty_as_u64();
        let target_registrations_per_interval: I65F63 = I65F63::from_num( Self::get_target_registrations_per_interval() ); // Target number of registrations on average over interval.
        log::trace!(
            target: LOG_TARGET,
            "current_difficulty: {:?}, max_difficulty: {:?}, min_difficulty: {:?}, adjustment_interval: {:?}, target_registrations_per_interval: {:?}",
            current_difficulty,
            max_difficulty,
            min_difficulty,
            adjustment_interval,
            target_registrations_per_interval
        );

        let current_block:u64 = Self::get_current_block_as_u64();
        let last_adjustment:u64 = LastDifficultyAdjustmentBlock::<T>::get();
        log::trace!(
            target: LOG_TARGET,
            "current_block: {:?}, last_adjustment: {:?}",
            current_block, last_adjustment
        );

        // --- Check if we have reached out adjustment interval.
        if current_block - last_adjustment >= adjustment_interval {

            // --- Compute average registrations over the adjustment interval.
            let registrations_since_last_adjustment: I65F63 = I65F63::from_num( Self::get_registrations_this_interval() );

            log::trace!(
                target: LOG_TARGET,
                "ADJUSTMENT REACHED: registrations_since_last_adjustment: {:?} ",
                registrations_since_last_adjustment
            );

            // --- Compare average against target.
            if registrations_since_last_adjustment > target_registrations_per_interval {

                // --- Double difficulty.
                let current_difficulty: u64 = Difficulty::<T>::get();
                let mut next_difficulty = current_difficulty * 2;
                if next_difficulty >= max_difficulty {
                    next_difficulty = max_difficulty
                }
                Self::set_difficulty_from_u64( next_difficulty );

                log::trace!(
                    target: LOG_TARGET,
                    "next_difficulty: {:?}",
                    next_difficulty,
                );

            } else {
                // --- Halve difficulty.
                let current_difficulty: u64 = Difficulty::<T>::get();
                let mut next_difficulty = current_difficulty / 2;
                if next_difficulty <= min_difficulty {
                    next_difficulty = min_difficulty
                }
                Self::set_difficulty_from_u64( next_difficulty );

                log::trace!(
                    target: LOG_TARGET,
                    "next_difficulty: {:?}",
                    next_difficulty,
                );
            }

            // --- Update last adjustment to current block and zero the registrations since last difficulty.
            LastDifficultyAdjustmentBlock::<T>::set( current_block );
            RegistrationsThisInterval::<T>::set( 0 );
        }

    }

    /// Block setup: Computation performed each block which updates the incentive mechanism and distributes new stake as dividends.
    /// 
    /// The following operations are performed in order.
    /// 
    /// 
    /// 
    /// ------ Requires ------:
    /// 
    /// Stake: 
    ///     -- S (Vec[n, u64])
    ///     -- s_i = tokens staked by peer i
    /// 
    /// Weights: 
    ///     -- W (Vec[n, Vec[n, u64]]): 
    ///     -- w_i = weights set by peer i
    ///     -- w_ij = weight set by peer i to peer j
    /// 
    /// Bonds: 
    ///     -- B (Vec[n, Vec[n, u64]]): 
    ///     -- b_i = bonds held by peer i
    ///     -- b_ij = bonds held by peer i in peer j
    /// 
    /// tau:
    ///     -- tau (u64):
    ///     -- tokens released this block.
    /// 
    /// 
    /// 
    /// ------ Computes ------:
    /// 
    /// Ranks: 
    ///    -- ranks Vec[u64] = R = (W^T * S)
    ///    -- r_i = SUM(j) s_j * w_ji
    ///    -- DB Reads/Writes: O( n^2 ), Decoding: O( n^2 ), Operations: O( n^2 )
    /// 
    /// Trust: 
    ///    -- trust Vec[u64] = T = (C^T * S) where c_ij = 1 iff w_ji != 0 else 0
    ///    -- t_i = SUM(j) s_j if w_ji != 0
    ///    -- DB Reads/Writes: O( n^2 ), Decoding: O( n^2 ), Operations: O( n^2 )
    /// 
    /// Incentive: 
    ///    -- incentive Vec[u64] = Icn = R * (exp(T) - 1)
    ///    -- icn_i = r_i * ( exp( t_i * temp ) - 1 ) )
    ///    -- DB Reads/Writes: O( 0 ), Decoding: O( 0 ), Operations: O( n )
    ///
    /// Inflation: 
    ///    -- inflation Vec[u64] = Inf = Icn * tau
    ///    -- inf_i = icn_i * tau
    ///    -- DB Reads/Writes: O( 0 ), Decoding: O( 0 ), Operations: O( n )
    /// 
    /// Dividends: 
    ///    -- dividends Vec[u64] = Div = B * Inf 
    ///    -- d_i = 0.5 * (SUM(j) b_ij * inf_j) + ( 0.5 * inf_i)
    ///    -- DB Reads/Writes: O( n^2 ), Decoding: O( n^2 ), Operations: O( n^2 )
    /// 
    /// 
    /// 
    /// ------ Updates ------:
    /// 
    /// Delta Stake:
    ///    -- S = S + D
    ///    -- s_i = s_i + d_i
    /// 
    /// Delta Bonds:
    ///    -- B = B + (W * S)
    ///    -- b_ij = b_ij + (w_ij * s_i)  
    ///
    /// 
    /// Note, operations 1 and 2 are computed together. 
    ////
    pub fn mechanism_step ( emission_this_step: u64 ) {

        // The amount this mechanism step emits on this block.
        let block_emission: I65F63 = I65F63::from_num( emission_this_step );
        log::trace!(
            target: LOG_TARGET,
            "step"
        );
      
        // Number of peers.
        let n: usize = Self::get_neuron_count() as usize;
        let block: u64 = Self::get_current_block_as_u64();
        
        // Constants.
        let activity_cutoff: u64 = Self::get_activity_cutoff();
        let bonds_moving_average:I65F63 = I65F63::from_num( Self::get_bonds_moving_average() ) / I65F63::from_num( 1_000_000 );
        let u64_max: I65F63 = I65F63::from_num( u64::MAX );
        let u32_max: I65F63 = I65F63::from_num( u32::MAX );
        let one: I65F63 = I65F63::from_num( 1.0 );
        let zero: I65F63 = I65F63::from_num( 0.0 );
        let rho: I65F63 = I65F63::from_num( Self::get_rho() );
        let kappa: I65F63 = one / I65F63::from_num( Self::get_kappa() );
        let self_ownership: I65F63 = one / I65F63::from_num( Self::get_self_ownership()  );

        // To be filled.
        let mut uids: Vec<u32> = vec![];
        let mut active: Vec<u32> = vec![0; n];
        let mut priority: Vec<u64> = vec![0;n];
        let mut bond_totals: Vec<u64> = vec![0; n];
        let mut bonds: Vec<Vec<u64>> = vec![vec![0;n]; n];
        let mut weights: Vec<Vec<(u32,u32)>> = vec![ vec![]; n ];
        let mut total_stake: I65F63 = I65F63::from_num( 0.0 );
        let mut total_active_stake: I65F63 = I65F63::from_num( 0.0 );
        let mut total_normalized_active_stake: I65F63 = I65F63::from_num( 0.0 );
        let mut stake: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        for ( uid_i, neuron_i ) in <Neurons<T> as IterableStorageMap<u32, NeuronMetadataOf<T>>>::iter() {

            // Append a set of uids.
            uids.push( uid_i );
            if block - neuron_i.last_update >= activity_cutoff {
                active [ uid_i as usize ] = 0;
            } else {
                active [ uid_i as usize ] = 1;
                total_active_stake += I65F63::from_num( neuron_i.stake );
            }
            total_stake += I65F63::from_num( neuron_i.stake );
            stake [ uid_i as usize ] = I65F63::from_num( neuron_i.stake );

            // Priority increments by the log of the stake and is drained everytime the account sets weights. 
            let log_stake:I65F63 = log2( I65F63::from_num( neuron_i.stake + 1 ) ).expect( "stake + 1 is positive and greater than 1.");
            priority [ uid_i as usize ] = neuron_i.priority + log_stake.to_num::<u64>();

            weights [ uid_i as usize ] = neuron_i.weights;             
            let mut bonds_row: Vec<u64> = vec![0; n];
            for (uid_j, bonds_ij) in neuron_i.bonds.iter() {
                
                // Prunning occurs here. We simply to do fill this bonds matrix 
                // with entries that contain the uids to prune. 
                if !NeuronsToPruneAtNextEpoch::<T>::contains_key(uid_j) {
                    // Otherwise, we add the entry into the stack based bonds array.
                    bonds_row [ *uid_j as usize ] = *bonds_ij;
                    bond_totals [ *uid_j as usize ] += *bonds_ij;
                }

            }
            bonds[ uid_i as usize ] = bonds_row;
        }
        // Normalize stake based on activity.
        if total_active_stake != 0 {
            for uid_i in uids.iter() {
                let normalized_active_stake:I65F63 = stake[ *uid_i as usize ] / total_active_stake;
                stake[ *uid_i as usize ] = normalized_active_stake;
                if active[ *uid_i as usize ] == 1 {
                    total_normalized_active_stake += normalized_active_stake;
                }
            }
        }
		log::trace!(
			target: LOG_TARGET,
			"stake: {:?}",
			stake
		);

        // Computational aspect starts here.
        
        // Compute ranks and trust.
        let mut total_bonds_purchased: u64 = 0;
        let mut total_ranks: I65F63 = I65F63::from_num( 0.0 );
        let mut total_trust: I65F63 = I65F63::from_num( 0.0 );
        let mut ranks: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        let mut trust: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        for uid_i in uids.iter() {

            // === Get vars for uid_i ===
            let stake_i: I65F63 = stake[ *uid_i as usize ];
            let weights_i: &Vec<(u32, u32)> = &weights[ *uid_i as usize ];
            if stake_i == zero { continue } // Skip zeros stake.

            // === Iterate over weights ===
            for ( uid_j, weight_ij ) in weights_i.iter() {

                if *uid_i == *uid_j { continue } // Skip self-weight.

                // === Compute score increments ===
                // Non active validators dont have the ability to increase ranks.
                // & bond increments converge to zero for non active validators.
                let mut rank_increment_ij: I65F63 = I65F63::from_num(0.0);
                let mut bond_increment_ij: I65F63 = I65F63::from_num(0.0);
                let mut trust_increment_ij: I65F63 = I65F63::from_num(0.0);
                if active[ *uid_i as usize ] == 1 { 
                    let weight_ij: I65F63 = I65F63::from_num( *weight_ij ) / u32_max; // Range( 0, 1 )
                    trust_increment_ij = stake_i; // Range( 0, 1 )                
                    rank_increment_ij = stake_i * weight_ij; // Range( 0, total_active_stake )
                    bond_increment_ij = rank_increment_ij * block_emission;
                }
                
                // === Increment neuron scores ===
                ranks[ *uid_j as usize ] += rank_increment_ij;  // Range( 0, total_active_stake )
                trust[ *uid_j as usize ] += trust_increment_ij;  // Range( 0, total_active_stake )
                total_ranks += rank_increment_ij;  // Range( 0, total_active_stake )
                total_trust += trust_increment_ij;  // Range( 0, total_active_stake )
                
                // === Compute bonding moving averages ===
                let prev_bonds_ij: I65F63 = I65F63::from_num( bonds[ *uid_i as usize  ][ *uid_j as usize ] );
                let moving_average_bonds_ij = bonds_moving_average * prev_bonds_ij + ( one - bonds_moving_average ) * bond_increment_ij;
                bonds [ *uid_i as usize  ][ *uid_j as usize ] = moving_average_bonds_ij.to_num::<u64>(); // Range( 0, block_emission )

                // === Update bond totals ===
                if prev_bonds_ij >= moving_average_bonds_ij {
                    bond_totals [ *uid_j as usize ] -= ( prev_bonds_ij - moving_average_bonds_ij ).to_num::<u64>(); // Range( 0, block_emission )
                    total_bonds_purchased = ( prev_bonds_ij - moving_average_bonds_ij ).to_num::<u64>(); // Range( 0, block_emission )
                } else {
                    bond_totals [ *uid_j as usize ] += ( moving_average_bonds_ij - prev_bonds_ij ).to_num::<u64>(); // Range( 0, block_emission )
                    total_bonds_purchased = ( moving_average_bonds_ij - prev_bonds_ij ).to_num::<u64>(); // Range( 0, block_emission )
                }
            }
        }
        // === Normalize ranks + trust ===
        if total_trust > 0 && total_ranks > 0 {
            for uid_i in uids.iter() {
                ranks[ *uid_i as usize ] = ranks[ *uid_i as usize ] / total_ranks; // Vector will sum to u64_max
                trust[ *uid_i as usize ] = trust[ *uid_i as usize ] / total_normalized_active_stake; // Vector will sum to u64_max
            }
        }
		 log::trace!(target: LOG_TARGET, "ranks: {:?}", ranks);
		 log::trace!(target: LOG_TARGET, "trust: {:?}", trust);
		 log::trace!(target: LOG_TARGET, "bonds: {:?}, {:?}, {:?}", bonds, bond_totals, total_bonds_purchased);

        // Compute consensus, incentive.
        let mut total_incentive: I65F63 = I65F63::from_num( 0.0 );
        let mut consensus: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        let mut incentive: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        if total_ranks != 0 && total_trust != 0 {
            for uid_i in uids.iter() {                    
                // Get exponentiated trust score.
                let trust_i: I65F63 = trust[ *uid_i as usize ];
                let shifted_trust: I65F63 = trust_i - kappa; // Range( -kappa, 1 - kappa )
                let temperatured_trust: I65F63 = shifted_trust * rho; // Range( -rho * kappa, rho ( 1 - kappa ) )
                let exponentiated_trust: I65F63 = exp( -temperatured_trust ).expect( "temperatured_trust is on range( -rho * kappa, rho ( 1 - kappa ) )"); // Range( exp(-rho * kappa), exp(rho ( 1 - kappa )) )
                  
                // Compute consensus.
                let ranks_i: I65F63 = ranks[ *uid_i as usize ];
                let consensus_i: I65F63 = one / (one + exponentiated_trust); // Range( 0, 1 )
                let incentive_i: I65F63 = ranks_i * consensus_i; // Range( 0, 1 )
                consensus[ *uid_i as usize ] = consensus_i; // Range( 0, 1 )
                incentive[ *uid_i as usize ] = incentive_i; // Range( 0, 1 )
                total_incentive += incentive_i;
            }
        }
        // Normalize Incentive.
        if total_incentive > 0 {
            for uid_i in uids.iter() {
                incentive[ *uid_i as usize ] = incentive[ *uid_i as usize ] / total_incentive; // Vector will sum to u64_max
            }
        }
        log::trace!(target: LOG_TARGET, "incentive: {:?}, consensus: {:?}", incentive, consensus);

        // Compute dividends.
        let mut total_dividends: I65F63 = I65F63::from_num( 0.0 );
        let mut dividends: Vec<I65F63> = vec![ I65F63::from_num( 0.0 ) ; n];
        let mut sparse_bonds: Vec<Vec<(u32,u64)>> = vec![vec![]; n];
        for uid_i in uids.iter() {

            // To be filled: Sparsified bonds.
            let mut sparse_bonds_row: Vec<(u32, u64)> = vec![];

            // Distribute dividends from self-ownership.
            let incentive_i: I65F63 = incentive[ *uid_i as usize ];
            let total_bonds_i: u64 = bond_totals[ *uid_i as usize ]; // Range( 0, total_emission );
            let mut dividends_ii: I65F63 = incentive_i * self_ownership;
            if total_bonds_i == 0 {
                dividends_ii += incentive_i * ( one - self_ownership ); // Add the other half.
            }
            dividends[ *uid_i as usize ] += dividends_ii; // Range( 0, block_emission / 2 );
            total_dividends += dividends_ii; // Range( 0, block_emission / 2 );

            // Distribute dividends from other-ownership.
            for uid_j in uids.iter() {
                
                // Get i -> j bonds.
                let bonds_ij: u64 = bonds[ *uid_i as usize ][ *uid_j as usize ]; // Range( 0, total_emission );
                let total_bonds_j: u64 = bond_totals[ *uid_j as usize ]; // Range( 0, total_emission );
                if total_bonds_j == 0 { continue; } // No bond ownership in this neuron.
                if bonds_ij == 0 { continue; } // No need to distribute dividends for zero bonds.

                // Compute bond fraction.
                let bond_fraction_ij: I65F63 = I65F63::from_num( bonds_ij ) / I65F63::from_num( total_bonds_j ); // Range( 0, 1 );

                // Compute incentive owenership fraction.
                let mut ownership_ji: I65F63 = one - self_ownership; // Range( 0, 1 );
                ownership_ji = ownership_ji * bond_fraction_ij; // Range( 0, 1 );

                // Compute dividends
                let dividends_ji: I65F63 = incentive[ *uid_j as usize ] * ownership_ji; // Range( 0, 1 );
                dividends[ *uid_i as usize ] += dividends_ji; // Range( 0, block_emission / 2 );
                total_dividends += dividends_ji; // Range( 0, block_emission / 2 );
                sparse_bonds_row.push( (*uid_j as u32, bonds_ij) );
            }
            sparse_bonds[ *uid_i as usize ] = sparse_bonds_row;
        }
        // Normalize dividends. Sanity check.
        let mut total_emission: u64 = 0;
        let mut emission: Vec<u64> = vec![ 0; n];
        if total_dividends != 0 {
            for uid_i in uids.iter() {
                let dividends_i: I65F63 = dividends[ *uid_i as usize ] / total_dividends;
                let emission_i: u64 = (block_emission * dividends_i).to_num::<u64>();
                dividends[ *uid_i as usize ] = dividends_i;
                emission[ *uid_i as usize ] = emission_i;
                total_emission += emission_i;
            }
        }

		 log::trace!(target: LOG_TARGET, "dividends: {:?}, emission: {:?}", dividends, emission);

        for ( uid_i, mut neuron_i ) in <Neurons<T> as IterableStorageMap<u32, NeuronMetadataOf<T>>>::iter() {
            // Update table entry.
            neuron_i.active = active[ uid_i as usize ];
            neuron_i.priority = priority[ uid_i as usize ];
            neuron_i.emission = emission[ uid_i as usize ];
            neuron_i.stake = neuron_i.stake + emission[ uid_i as usize ];
            neuron_i.rank = (ranks[ uid_i as usize ] * u64_max).to_num::<u64>();
            neuron_i.trust = (trust[ uid_i as usize ] * u64_max).to_num::<u64>();
            neuron_i.consensus = (consensus[ uid_i as usize ] * u64_max).to_num::<u64>();
            neuron_i.incentive = (incentive[ uid_i as usize ] * u64_max).to_num::<u64>();
            neuron_i.dividends = (dividends[ uid_i as usize ] * u64_max).to_num::<u64>();
            neuron_i.bonds = sparse_bonds[ uid_i as usize ].clone();
            Neurons::<T>::insert( neuron_i.uid, neuron_i );

            // This where we remove the neurons to prune (clearing the table.)
            if NeuronsToPruneAtNextEpoch::<T>::contains_key( uid_i ) {
                NeuronsToPruneAtNextEpoch::<T>::remove ( uid_i );
            }
        }

        // Amount distributed through mechanism in conjunction with amount distributed to foudation.
        let total_new_issuance:u64 = total_emission; // + foundation_distribution_as_float.to_num::<u64>();

        // Update totals.
        TotalEmission::<T>::set( total_emission );
        TotalBondsPurchased::<T>::set( total_bonds_purchased );
        TotalIssuance::<T>::mutate( |val| *val += total_new_issuance );
        TotalStake::<T>::mutate( |val| *val += total_emission );
        LastMechansimStepBlock::<T>::set( block );
    }

    pub fn get_current_block_as_u64( ) -> u64 {
        let block_as_u64: u64 = TryInto::try_into( system::Pallet::<T>::block_number() ).ok().expect("blockchain will not exceed 2^64 blocks; QED.");
        block_as_u64
    }

    pub fn reset_bonds( ) {
        for ( _, mut neuron_i ) in <Neurons<T> as IterableStorageMap<u32, NeuronMetadataOf<T>>>::iter() {
            neuron_i.bonds = vec![];
            Neurons::<T>::insert( neuron_i.uid, neuron_i );
        }
    }

}